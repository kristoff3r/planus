use planus::ReadAsRoot;

fn read_u32(data: &[u8], position: usize) -> usize {
    u32::from_le_bytes(data[position..position + 4].try_into().unwrap()) as usize
}

fn read_u16(data: &[u8], position: usize) -> usize {
    u16::from_le_bytes(data[position..position + 2].try_into().unwrap()) as usize
}

// Follows the uoffset stored at `position`.
fn follow(data: &[u8], position: usize) -> usize {
    position + read_u32(data, position)
}

// The position of the field in vtable slot `slot` of the table at `table`.
fn field(data: &[u8], table: usize, slot: usize) -> usize {
    let vtable_offset = i32::from_le_bytes(data[table..table + 4].try_into().unwrap());
    let vtable = (table as isize - vtable_offset as isize) as usize;
    assert!(slot + 2 <= read_u16(data, vtable), "slot {} missing", slot);
    let offset = read_u16(data, vtable + slot);
    assert_ne!(offset, 0, "slot {} not set", slot);
    table + offset
}

// The positions of the elements of the vector of offsets at `vector`.
fn offset_elements(data: &[u8], vector: usize) -> Vec<usize> {
    (0..read_u32(data, vector))
        .map(|i| follow(data, vector + 4 + 4 * i))
        .collect()
}

#[track_caller]
fn assert_aligned(position: usize, alignment: usize, what: &str, case: &str) {
    assert_eq!(
        position % alignment,
        0,
        "{case}: {what} at {position} is not {alignment}-aligned"
    );
}

fn union_alignment(tag: u8) -> usize {
    match tag {
        1 => 8,  // Struct64
        2 => 16, // Struct16
        3 => 4,  // Inner
        4 => 4,  // String
        _ => panic!("unexpected tag {}", tag),
    }
}

fn check_alignment(data: &[u8], case: &str) {
    let root = read_u32(data, 0);
    assert_aligned(root, 4, "root", case);

    // The element data of a vector starts 4 bytes after the offset target.
    for (slot, alignment, what) in [
        (6, 8, "uint64s"),
        (8, 8, "float64s"),
        (10, 8, "struct64s"),
        (12, 16, "struct16s"),
    ] {
        let vector = follow(data, field(data, root, slot));
        assert_aligned(vector, 4, what, case);
        assert_aligned(vector + 4, alignment, what, case);
    }

    let tables = follow(data, field(data, root, 14));
    for table in offset_elements(data, tables) {
        assert_aligned(table, 4, "tables[_]", case);
        assert_aligned(field(data, table, 6), 8, "tables[_].value", case);
    }

    let tag = data[field(data, root, 16)];
    let union = follow(data, field(data, root, 18));
    assert_aligned(union, union_alignment(tag), "union", case);

    let tags = follow(data, field(data, root, 20));
    let values = follow(data, field(data, root, 22));
    assert_aligned(tags, 4, "unions tags", case);
    assert_aligned(values, 4, "unions values", case);
    for (i, value) in offset_elements(data, values).into_iter().enumerate() {
        let tag = data[tags + 4 + i];
        assert_aligned(value, union_alignment(tag), "unions[_]", case);
    }

    assert_aligned(field(data, root, 24), 16, "struct16", case);
    assert_aligned(field(data, root, 26), 8, "value", case);
}

// The flatbuffers verifier checks the alignment of scalar vectors and scalar
// fields, but sees structs as byte arrays.
fn verify(data: &[u8]) -> Result<(), flatbuffers::InvalidFlatbuffer> {
    use flatbuffers::{ForwardsUOffset, Vector};
    let opts = flatbuffers::VerifierOptions::default();
    let mut verifier = flatbuffers::Verifier::new(&opts, data);
    let root = verifier.get_uoffset(0)? as usize;
    verifier
        .visit_table(root)?
        .visit_field::<ForwardsUOffset<Vector<u8>>>("bytes", 4, false)?
        .visit_field::<ForwardsUOffset<Vector<u64>>>("uint64s", 6, false)?
        .visit_field::<ForwardsUOffset<Vector<f64>>>("float64s", 8, false)?
        .visit_field::<u64>("value", 26, false)?
        .finish();
    Ok(())
}

let uint64s = [1u64, 0x0102_0304_0506_0708, u64::MAX];
let float64s = [1.5f64, -2.25];
let struct64s = [Struct64 { value: 7 }, Struct64 { value: u64::MAX }];
let struct16s = [Struct16 { value: 1 }, Struct16 { value: 2 }, Struct16 { value: 3 }];
let tables = [Inner { a: 1, value: 2 }, Inner { a: 3, value: 4 }];
let unions = [
    Union::Struct64(Struct64 { value: 5 }),
    Union::String("x".into()),
    Union::Struct16(Struct16 { value: 6 }),
    Union::Inner(Box::new(Inner { a: 7, value: 8 })),
    Union::Struct64(Struct64 { value: 9 }),
];
let all_bytes = [0xAAu8; 16];

type Build = fn(&mut planus::Builder, &[u8], Union) -> planus::Offset<Root>;
let builds: [(&str, Build); 3] = [
    ("array", |builder, bytes, union| {
        Root::create(
            builder,
            bytes,
            [1u64, 0x0102_0304_0506_0708, u64::MAX],
            [1.5f64, -2.25],
            [Struct64 { value: 7 }, Struct64 { value: u64::MAX }],
            [Struct16 { value: 1 }, Struct16 { value: 2 }, Struct16 { value: 3 }],
            [Inner { a: 1, value: 2 }, Inner { a: 3, value: 4 }],
            union,
            [
                Union::Struct64(Struct64 { value: 5 }),
                Union::String("x".into()),
                Union::Struct16(Struct16 { value: 6 }),
                Union::Inner(Box::new(Inner { a: 7, value: 8 })),
                Union::Struct64(Struct64 { value: 9 }),
            ],
            Struct16 { value: 10 },
            11u64,
        )
    }),
    ("slice", |builder, bytes, union| {
        Root::create(
            builder,
            bytes,
            &[1u64, 0x0102_0304_0506_0708, u64::MAX][..],
            &[1.5f64, -2.25][..],
            &[Struct64 { value: 7 }, Struct64 { value: u64::MAX }][..],
            &[Struct16 { value: 1 }, Struct16 { value: 2 }, Struct16 { value: 3 }][..],
            &[Inner { a: 1, value: 2 }, Inner { a: 3, value: 4 }][..],
            union,
            &[
                Union::Struct64(Struct64 { value: 5 }),
                Union::String("x".into()),
                Union::Struct16(Struct16 { value: 6 }),
                Union::Inner(Box::new(Inner { a: 7, value: 8 })),
                Union::Struct64(Struct64 { value: 9 }),
            ][..],
            Struct16 { value: 10 },
            11u64,
        )
    }),
    ("vec", |builder, bytes, union| {
        Root::create(
            builder,
            bytes,
            vec![1u64, 0x0102_0304_0506_0708, u64::MAX],
            vec![1.5f64, -2.25],
            vec![Struct64 { value: 7 }, Struct64 { value: u64::MAX }],
            vec![Struct16 { value: 1 }, Struct16 { value: 2 }, Struct16 { value: 3 }],
            vec![Inner { a: 1, value: 2 }, Inner { a: 3, value: 4 }],
            union,
            vec![
                Union::Struct64(Struct64 { value: 5 }),
                Union::String("x".into()),
                Union::Struct16(Struct16 { value: 6 }),
                Union::Inner(Box::new(Inner { a: 7, value: 8 })),
                Union::Struct64(Struct64 { value: 9 }),
            ],
            Struct16 { value: 10 },
            11u64,
        )
    }),
];

// One builder is reused across all cases, so every case also checks that
// `clear` resets the alignment state.
let mut builder = planus::Builder::new();
for union in unions.iter() {
    for padding in 0..all_bytes.len() {
        let bytes = &all_bytes[..padding];
        let mut previous: Option<Vec<u8>> = None;
        for (name, build) in builds {
            builder.clear();
            let root = build(&mut builder, bytes, union.clone());
            let data = builder.finish(root, None);
            let case = format!("{name}, {padding} bytes, union {union:?}");

            verify(data).unwrap_or_else(|err| panic!("{}: {}", case, err));
            check_alignment(data, &case);

            let root: Root = RootRef::read_as_root(data).unwrap().try_into().unwrap();
            assert_eq!(root.bytes.as_deref(), Some(bytes));
            assert_eq!(root.uint64s.as_deref(), Some(&uint64s[..]));
            assert_eq!(root.float64s.as_deref(), Some(&float64s[..]));
            assert_eq!(root.struct64s.as_deref(), Some(&struct64s[..]));
            assert_eq!(root.struct16s.as_deref(), Some(&struct16s[..]));
            assert_eq!(root.tables.as_deref(), Some(&tables[..]));
            assert_eq!(root.union_.as_ref(), Some(union));
            assert_eq!(root.unions.as_deref(), Some(&unions[..]));
            assert_eq!(root.struct16, Some(Struct16 { value: 10 }));
            assert_eq!(root.value, 11);

            if let Some(previous) = &previous {
                assert_eq!(previous.as_slice(), data, "{case}");
            }
            previous = Some(data.to_vec());
        }
    }
}

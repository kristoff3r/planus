use alloc::collections::BTreeMap;

use planus::{Builder, Offset, ReadAsRoot, UnionOffset, UnionVectorOffset};

// The destructuring is exhaustive, so this also checks that the deprecated
// field has no key.
check_type!(NodeKey => {
    flag: bool,
    count: u32,
    maybe_count: Option<u32>,
    color: Color,
    maybe_color: Option<Color>,
    weight: u32,
    maybe_weight: Option<u64>,
    segment: Option<SegmentKey>,
    maybe_point: Option<PointKey>,
    cell: Cell,
    name: Option<Offset<str>>,
    default_name: Offset<str>,
    required_name: Offset<str>,
    leaf: Option<Offset<Leaf>>,
    required_leaf: Offset<Leaf>,
    leaves: Option<Offset<[Offset<Leaf>]>>,
    numbers: Option<Offset<[u32]>>,
    default_numbers: Offset<[u32]>,
    points: Option<Offset<[Point]>>,
    names: Option<Offset<[Offset<str>]>>,
    child: Option<UnionOffset<Child>>,
    required_child: UnionOffset<Child>,
    children: Option<UnionVectorOffset<Child>>,
});
check_type!(PointKey => { x: u32, y: u64 });
check_type!(SegmentKey => { start: PointKey, cell: Cell });
check_type!(LeafKey => { value: i32 });
check_type!(EmptyKey => {});
check_type!(NodeKey => create(&NodeKey, &mut Builder) : Offset<Node>);
check_type!(NodeKey => impl planus::WriteAsOffset<Node>);
check_type!(NodeKey => impl planus::WriteAs<Offset<Node>>);
check_type!(NodeKey => impl planus::WriteAsOptional<Offset<Node>>);

assert_traits!(
    NodeKey: Copy + Clone + Debug + PartialEq + PartialOrd + Eq + Ord + Hash + !Default,
    EmptyKey: Copy + Clone + Debug + Eq + Ord + Hash,
    PointKey: Copy + Clone + Debug + Eq + Ord + Hash + {From<Point>} + {Into<Point>},
    SegmentKey: Copy + Clone + Debug + Eq + Ord + Hash + {From<Segment>} + {Into<Segment>},
);

// `Cell` implements `Hash` and `Eq` itself.
let _: CellKey = Cell {
    index: 0,
    color: Color::Red,
};

// Floats compare by their bits.
let point = |x: f32| Point { x, y: 0.5 };
assert_eq!(PointKey::from(point(1.0)), PointKey::from(point(1.0)));
assert_ne!(PointKey::from(point(0.0)), PointKey::from(point(-0.0)));
assert_eq!(PointKey::from(point(f32::NAN)), PointKey::from(point(f32::NAN)));
assert_eq!(Point::from(PointKey::from(point(-0.0))).x.to_bits(), (-0.0f32).to_bits());

let segment = Segment {
    start: point(2.5),
    cell: Cell {
        index: 3,
        color: Color::Blue,
    },
};
assert_eq!(Segment::from(SegmentKey::from(segment)), segment);

let mut builder = Builder::new();

// Equal keys can share one table.
let mut leaves: BTreeMap<LeafKey, Offset<Leaf>> = BTreeMap::new();
let mut share_leaf = |builder: &mut Builder, value| {
    *leaves
        .entry(LeafKey { value })
        .or_insert_with_key(|key| key.create(builder))
};
let leaf_1 = share_leaf(&mut builder, 1);
let leaf_2 = share_leaf(&mut builder, 2);
let len = builder.len();
assert_eq!(share_leaf(&mut builder, 1), leaf_1);
assert_eq!(builder.len(), len);
assert_ne!(leaf_1, leaf_2);

let name = builder.create_string("name");
let required_name = builder.create_string("required");
let default_name = builder.create_string("default");
let leaf_vector = builder.create_vector([leaf_1, leaf_2, leaf_1]);
let numbers = builder.create_vector([1u32, 2, 3]);
let default_numbers = builder.create_vector::<u32>(&[] as &[u32]);
let points = builder.create_vector([point(1.0)]);
let names = builder.create_vector([name, required_name]);
let child = Child::create_leaf(&mut builder, leaf_2);
let required_child = Child::create_name(&mut builder, name);
let children = builder.create_union_vector([
    Child::Point(point(4.0)),
    Child::Name("child".into()),
]);

let key = NodeKey {
    flag: false,
    count: 7,
    maybe_count: Some(8),
    color: Color::Blue,
    maybe_color: None,
    weight: 2.25f32.to_bits(),
    maybe_weight: Some((-0.0f64).to_bits()),
    segment: Some(segment.into()),
    maybe_point: None,
    cell: segment.cell,
    name: Some(name),
    default_name,
    required_name,
    leaf: Some(leaf_1),
    required_leaf: leaf_2,
    leaves: Some(leaf_vector),
    numbers: Some(numbers),
    default_numbers,
    points: Some(points),
    names: Some(names),
    child: Some(child),
    required_child,
    children: Some(children),
};
let node = key.create(&mut builder);
let data = builder.finish(node, None);

let node: Node = NodeRef::read_as_root(data).unwrap().try_into().unwrap();
let leaf = |value| Box::new(Leaf { value });
let expected = Node {
    flag: false,
    count: 7,
    maybe_count: Some(8),
    color: Color::Blue,
    maybe_color: None,
    weight: 2.25,
    maybe_weight: Some(-0.0),
    segment: Some(segment),
    maybe_point: None,
    cell: segment.cell,
    name: Some("name".into()),
    default_name: "default".into(),
    required_name: "required".into(),
    leaf: Some(leaf(1)),
    required_leaf: leaf(2),
    leaves: Some(vec![Leaf { value: 1 }, Leaf { value: 2 }, Leaf { value: 1 }]),
    numbers: Some(vec![1, 2, 3]),
    default_numbers: vec![],
    points: Some(vec![point(1.0)]),
    names: Some(vec!["name".into(), "required".into()]),
    child: Some(Child::Leaf(leaf(2))),
    required_child: Child::Name("name".into()),
    children: Some(vec![Child::Point(point(4.0)), Child::Name("child".into())]),
};
assert_eq!(node, expected);
assert_eq!(node.maybe_weight.unwrap().to_bits(), (-0.0f64).to_bits());

let mut builder = Builder::new();
let empty = EmptyKey {}.create(&mut builder);
let data = builder.finish(empty, None);
let _: Empty = EmptyRef::read_as_root(data).unwrap().try_into().unwrap();

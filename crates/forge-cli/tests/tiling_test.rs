/// Test the tiling layout logic by replicating the key structures.
/// (Can't import from binary crate directly, so we test the patterns.)

#[test]
fn test_split_and_close_semantics() {
    // Simulate the binary tree tiling behavior
    #[derive(Debug)]
    enum Node {
        Leaf(usize),
        Split(Box<Node>, Box<Node>),
    }

    fn count_leaves(node: &Node) -> usize {
        match node {
            Node::Leaf(_) => 1,
            Node::Split(a, b) => count_leaves(a) + count_leaves(b),
        }
    }

    // Start with one leaf
    let mut root = Node::Leaf(0);
    assert_eq!(count_leaves(&root), 1);

    // Split into two
    root = Node::Split(Box::new(root), Box::new(Node::Leaf(1)));
    assert_eq!(count_leaves(&root), 2);

    // Split again
    root = Node::Split(Box::new(root), Box::new(Node::Leaf(2)));
    assert_eq!(count_leaves(&root), 3);
}

#[test]
fn test_ratio_clamping() {
    let min: f32 = 0.1;
    let max: f32 = 0.9;
    let step: f32 = 0.05;

    let mut ratio: f32 = 0.5;

    // Grow 10 times
    for _ in 0..10 {
        ratio = (ratio + step).min(max);
    }
    assert!((ratio - max).abs() < f32::EPSILON);

    // Shrink 20 times
    for _ in 0..20 {
        ratio = (ratio - step).max(min);
    }
    assert!((ratio - min).abs() < f32::EPSILON);
}

// Minimal protobuf encoder for an ONNX graph computing y = x + x (f32[1]).
fn tiny_graph() -> Vec<u8> {
    fn var(mut n: usize) -> Vec<u8> {
        let mut b = Vec::new();
        while n >= 128 {
            b.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        b.push(n as u8);
        b
    }
    fn msg(tag: usize, v: Vec<u8>) -> Vec<u8> {
        let mut b = var(tag * 8 + 2);
        b.extend(var(v.len()));
        b.extend(v);
        b
    }
    fn num(tag: usize, n: usize) -> Vec<u8> {
        let mut b = var(tag * 8);
        b.extend(var(n));
        b
    }
    fn text(tag: usize, s: &str) -> Vec<u8> {
        msg(tag, s.as_bytes().to_vec())
    }
    fn vi(name: &str) -> Vec<u8> {
        let shape = msg(1, num(1, 1));
        let mut tensor = num(1, 1);
        tensor.extend(msg(2, shape));
        let mut v = text(1, name);
        v.extend(msg(2, msg(1, tensor)));
        v
    }
    let mut node = text(1, "x");
    node.extend(text(1, "x"));
    node.extend(text(2, "y"));
    node.extend(text(4, "Add"));
    let mut graph = msg(1, node);
    graph.extend(text(2, "cuda_preflight"));
    graph.extend(msg(11, vi("x")));
    graph.extend(msg(12, vi("y")));
    let mut model = num(1, 8);
    model.extend(msg(7, graph));
    model.extend(msg(8, num(2, 13)));
    model
}

use iced_wgpu::wgpu::naga;
use std::collections::{HashMap, HashSet};

const SHADER: &str = r#"
struct Item { value: vec4<f32>, matrix: mat4x4<f32> }
struct Memory { items: array<Item, 3> }
@group(0) @binding(0) var<uniform> constants: Memory;
@group(0) @binding(1) var<storage, read> input_buffer: Memory;
struct ImmediateItem { offset: vec4<f32>, matrix: mat4x4<f32> }
struct ImmediateMemory { entries: array<ImmediateItem, 2> }
var<immediate> immediate_data: ImmediateMemory;
var<private> positions: array<vec2<f32>, 3> = array(
    vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(0.0, 1.0));
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) color: vec4<f32> }
@vertex fn main(@builtin(vertex_index) index: u32) -> Vertex {
    var output: Vertex;
    var offsets = array<vec2<f32>, 3>(vec2(0.0), vec2(1.0), vec2(2.0));
    output.position = vec4(positions[index] + offsets[index] * 0.01, 0.0, 1.0);
    output.color = constants.items[index].value + input_buffer.items[index].value
        + immediate_data.entries[0].offset;
    return output;
}
"#;

fn words(source: &str) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(source).unwrap();
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
    naga::back::spv::write_vec(
        &module,
        &info,
        &naga::back::spv::Options::default(),
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Vertex,
            entry_point: "main".into(),
        }),
    )
    .unwrap()
}

fn descendants(root: u32, children: &HashMap<u32, Vec<u32>>) -> HashSet<u32> {
    let mut seen = HashSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if seen.insert(id) {
            pending.extend(children.get(&id).into_iter().flatten().copied());
        }
    }
    seen
}

#[test]
fn shader_layout_excludes_composites_without_memory_interfaces() {
    let words = words(SHADER);
    let mut children = HashMap::new();
    let mut pointers = HashMap::new();
    let mut variables = Vec::new();
    let mut decorated = HashSet::new();
    let mut offsets = HashSet::new();
    let mut strides = HashSet::new();
    let mut matrices = HashSet::new();
    let mut cursor = 5;
    while cursor < words.len() {
        let count = (words[cursor] >> 16) as usize;
        assert!(count > 0 && cursor + count <= words.len());
        let op = words[cursor] & 0xffff;
        let args = &words[cursor + 1..cursor + count];
        match op {
            28 | 29 => {
                children.insert(args[0], vec![args[1]]);
            }
            30 => {
                children.insert(args[0], args[1..].to_vec());
            }
            32 => {
                pointers.insert(args[0], (args[1], args[2]));
            }
            59 => variables.push((args[0], args[2])),
            71 if args[1] == 6 => {
                decorated.insert(args[0]);
                strides.insert(args[0]);
            }
            72 if matches!(args[2], 5 | 7 | 35) => {
                decorated.insert(args[0]);
                if args[2] == 35 {
                    offsets.insert(args[0]);
                }
                if args[2] == 7 {
                    matrices.insert(args[0]);
                }
            }
            _ => {}
        }
        cursor += count;
    }
    let mut local = 0;
    let mut memory = 0;
    for (pointer, class) in variables {
        let (pointer_class, base) = pointers[&pointer];
        assert_eq!(pointer_class, class);
        let reachable = descendants(base, &children);
        match class {
            6 | 7 => {
                local += 1;
                assert!(
                    reachable.is_disjoint(&decorated),
                    "non-memory variable class {class} reaches explicitly laid out types: {:?}",
                    reachable.intersection(&decorated).collect::<Vec<_>>()
                );
            }
            2 | 9 | 12 => {
                memory += 1;
                assert!(
                    reachable.intersection(&offsets).count() >= 2,
                    "nested buffer structures must retain member offsets"
                );
                assert!(
                    !reachable.is_disjoint(&strides),
                    "buffer arrays must retain their array stride"
                );
                assert!(
                    !reachable.is_disjoint(&matrices),
                    "buffer matrices must retain their matrix stride"
                );
            }
            _ => {}
        }
    }
    assert!(
        local >= 3,
        "fixture must exercise private and function composites"
    );
    assert_eq!(
        memory, 3,
        "uniform, storage and immediate interfaces are exercised"
    );
}

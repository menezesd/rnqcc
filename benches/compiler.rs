use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use rnqcc::compile::{compile, CompatibilityOptions, CompileOptions, DumpOptions, WarningOptions};
use rnqcc::optimize::{optimize_program, OptimizationFlags};
use rnqcc::tempfile::TempFile;
use rnqcc::types::{Stage, Target};
use rnqcc::{lex, parse, resolve, tacky};
use std::fs;
use std::hint::black_box;

fn lower_and_optimize(source: &str) {
    let tokens = lex::lex(black_box(source)).expect("lex failed");
    let ast = parse::parse(tokens).expect("parse failed");
    let resolved = resolve::resolve(ast).expect("resolve failed").program;
    let mut program = tacky::generate(resolved).expect("tacky generation failed");
    optimize_program(&mut program, &OptimizationFlags::all_enabled());
    black_box(program);
}

fn build_tacky(source: &str) -> rnqcc::types::TackyProgram {
    let tokens = lex::lex(black_box(source)).expect("lex failed");
    let ast = parse::parse(tokens).expect("parse failed");
    let resolved = resolve::resolve(ast).expect("resolve failed").program;
    tacky::generate(resolved).expect("tacky generation failed")
}

fn optimize_tacky(program: &mut rnqcc::types::TackyProgram) {
    optimize_program(program, &OptimizationFlags::all_enabled());
}

fn compile_to_assembly(source: &str, target: Target) {
    let mut src_path = std::env::temp_dir();
    src_path.push(format!(
        "rnqcc-bench-{}-{}-{}.c",
        std::process::id(),
        std::thread::current().name().unwrap_or("bench"),
        target.triple_name()
    ));
    let asm_path = src_path.with_extension("s");
    let _src_guard = TempFile::new(&src_path);
    let _asm_guard = TempFile::new(&asm_path);
    fs::write(&src_path, black_box(source)).expect("write benchmark source");
    compile(
        &Stage::Assembly,
        src_path.to_str().expect("temp source path"),
        CompileOptions {
            target: &target,
            opt_flags: &OptimizationFlags::all_enabled(),
            no_coalescing: false,
            instrument_functions: false,
            compatibility: CompatibilityOptions::default(),
            dumps: DumpOptions::default(),
            warnings: WarningOptions::default(),
        },
    )
    .expect("full compile failed");
    black_box(asm_path);
}

fn generated_struct_workload() -> String {
    let mut source = String::from(
        "
struct Pair {
    long x;
    long y;
};

static long sink;

long mix(struct Pair p, long scale) {
    long acc = 0;
",
    );
    for i in 0..96 {
        source.push_str(&format!(
            "    acc = acc + (p.x + {i}) * (scale - {i}) + p.y;\n"
        ));
    }
    source.push_str(
        "
    sink = acc;
    return acc;
}

int main(void) {
    struct Pair p = { 17, 23 };
    return (int)mix(p, 11);
}
",
    );
    source
}

fn arithmetic_workload() -> &'static str {
    "
int hot(int n) {
    int acc = 0;
    for (int i = 0; i < n; i = i + 1) {
        int a = i * 13 + 7;
        int b = a ^ (i << 2);
        if ((b & 3) == 0) {
            acc = acc + b / 3;
        } else {
            acc = acc - b;
        }
    }
    return acc;
}

int main(void) {
    return hot(1000);
}
"
}

fn call_heavy_workload() -> &'static str {
    "
static int sink;

static int twist(int x) {
    return (x * 17) ^ (x >> 2);
}

int reduce(int n) {
    int acc = 0;
    for (int i = 0; i < n; i = i + 1) {
        int t = twist(i);
        if ((t & 7) == 0) {
            acc = acc + t;
        } else {
            acc = acc - (t >> 1);
        }
    }
    sink = acc;
    return acc;
}

int main(void) {
    return reduce(400);
}
"
}

fn compiler_benches(c: &mut Criterion) {
    let struct_source = generated_struct_workload();
    let arithmetic_tacky = build_tacky(arithmetic_workload());
    let struct_tacky = build_tacky(&struct_source);
    c.bench_function("front_middle_arithmetic", |b| {
        b.iter(|| lower_and_optimize(arithmetic_workload()))
    });
    c.bench_function("front_middle_generated_structs", |b| {
        b.iter(|| lower_and_optimize(&struct_source))
    });
    c.bench_function("optimize_only_arithmetic", |b| {
        b.iter_batched(
            || arithmetic_tacky.clone(),
            |mut program| {
                optimize_tacky(&mut program);
                black_box(program);
            },
            BatchSize::SmallInput,
        )
    });
    c.bench_function("optimize_only_generated_structs", |b| {
        b.iter_batched(
            || struct_tacky.clone(),
            |mut program| {
                optimize_tacky(&mut program);
                black_box(program);
            },
            BatchSize::SmallInput,
        )
    });
    c.bench_function("full_compile_call_heavy", |b| {
        b.iter(|| compile_to_assembly(call_heavy_workload(), Target::host()))
    });
    c.bench_function("full_compile_call_heavy_aarch64", |b| {
        b.iter(|| compile_to_assembly(call_heavy_workload(), Target::aarch64_linux()))
    });
}

criterion_group!(benches, compiler_benches);
criterion_main!(benches);

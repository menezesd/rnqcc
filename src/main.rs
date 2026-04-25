use clap::{App, Arg};

mod codegen;
mod compile;
mod emit;
mod lex;
mod optimize;
mod parse;
mod resolve;
mod tacky;
mod types;

use crate::types::*;

use std::path::Path;
use std::process::Command;

fn current_platform() -> Platform {
    let uname_output =
        String::from_utf8_lossy(&Command::new("uname").output().unwrap().stdout).to_lowercase();
    if uname_output.starts_with("darwin") {
        Platform::OsX
    } else {
        Platform::Linux
    }
}

fn validate_extension(filename: &str) {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if ext != "c" && ext != "h" {
        panic!("Expected C source file with .c or .h extension");
    }
}

fn replace_extension(filename: &str, new_extension: &str) -> String {
    let path = Path::new(filename);
    path.with_extension(new_extension)
        .to_str()
        .unwrap()
        .to_string()
}

fn run_command(cmd: &str) {
    let status = Command::new("sh").arg("-c").arg(cmd).status().unwrap();
    if !status.success() {
        panic!("Command failed: {}", cmd);
    }
}

fn preprocess(src: &str) -> String {
    validate_extension(src);
    let output = replace_extension(src, "i");
    let preprocess_cmd = format!("gcc -E -P {} -o {}", src, output);
    run_command(&preprocess_cmd);
    output
}

fn do_compile(stage: &Stage, preprocessed_src: &str, target: &Platform, opt_flags: &optimize::OptimizationFlags) -> String {
    compile::compile(stage, preprocessed_src, target, opt_flags);
    let _ = std::fs::remove_file(preprocessed_src);
    replace_extension(preprocessed_src, "s")
}

fn assemble_and_link(asm_files: &[String], output: &str, target: &Platform, cleanup: bool) {
    let arch_flag = match target {
        Platform::OsX => " -arch x86_64",
        Platform::Linux => "",
    };
    let files = asm_files.join(" ");
    let assemble_cmd = format!("gcc{} {} -o {}", arch_flag, files, output);
    run_command(&assemble_cmd);

    if cleanup {
        for f in asm_files {
            let _ = std::fs::remove_file(f);
        }
    }
}

fn driver(target: Platform, debug: bool, stage: Stage, sources: &[&str], opt_flags: &optimize::OptimizationFlags) {
    let mut asm_files = Vec::new();

    for src in sources {
        let preprocessed_name = preprocess(src);
        let assembly_name = do_compile(&stage, &preprocessed_name, &target, opt_flags);
        asm_files.push(assembly_name);
    }

    if stage == Stage::Object {
        // Assemble each .s to .o
        for asm_file in &asm_files {
            let obj_file = replace_extension(asm_file, "o");
            let arch_flag = match target {
                Platform::OsX => " -arch x86_64",
                Platform::Linux => "",
            };
            run_command(&format!("gcc{} -c {} -o {}", arch_flag, asm_file, obj_file));
            if !debug {
                let _ = std::fs::remove_file(asm_file);
            }
        }
    } else if stage == Stage::Executable {
        // Output name is based on the first source file
        let output_file = Path::new(sources[0])
            .with_extension("")
            .to_str()
            .unwrap_or("a.out")
            .to_string();
        assemble_and_link(&asm_files, &output_file, &target, !debug);
    }
}

fn main() {
    let matches = App::new("rnqcc")
        .version("0.2.0")
        .author("Dean Menezes")
        .about("A not-quite-C compiler")
        .arg(
            Arg::with_name("stage")
                .long("stage")
                .takes_value(true)
                .possible_values(["lex", "parse", "validate", "tacky", "codegen", "s"])
                .help("Run the specified compiler stage"),
        )
        .arg(
            Arg::with_name("emit_asm")
                .short('S')
                .takes_value(false)
                .help("Emit assembly (like gcc -S)"),
        )
        .arg(
            Arg::with_name("compile_only")
                .short('c')
                .takes_value(false)
                .help("Compile to object file (like gcc -c)"),
        )
        .arg(
            Arg::with_name("target")
                .short('t')
                .long("target")
                .takes_value(true)
                .possible_values(["linux", "osx"])
                .help("Choose target platform"),
        )
        .arg(
            Arg::with_name("debug")
                .short('d')
                .long("debug")
                .takes_value(false)
                .help("Write out debug information"),
        )
        .arg(
            Arg::with_name("fold_constants")
                .long("fold-constants")
                .takes_value(false)
                .help("Enable constant folding optimization"),
        )
        .arg(
            Arg::with_name("eliminate_unreachable_code")
                .long("eliminate-unreachable-code")
                .takes_value(false)
                .help("Enable unreachable code elimination"),
        )
        .arg(
            Arg::with_name("propagate_copies")
                .long("propagate-copies")
                .takes_value(false)
                .help("Enable copy propagation"),
        )
        .arg(
            Arg::with_name("eliminate_dead_stores")
                .long("eliminate-dead-stores")
                .takes_value(false)
                .help("Enable dead store elimination"),
        )
        .arg(
            Arg::with_name("optimize")
                .long("optimize")
                .takes_value(false)
                .help("Enable all optimizations"),
        )
        .arg(
            Arg::with_name("src_files")
                .index(1)
                .required(true)
                .multiple(true)
                .help("Input file(s)"),
        )
        .get_matches();

    let stage = if matches.is_present("emit_asm") {
        Stage::Assembly
    } else if matches.is_present("compile_only") {
        Stage::Object
    } else {
        match matches.value_of("stage") {
            Some("lex") => Stage::Lex,
            Some("parse") => Stage::Parse,
            Some("validate") => Stage::Validate,
            Some("tacky") => Stage::Tacky,
            Some("codegen") => Stage::Codegen,
            Some("s") => Stage::Assembly,
            _ => Stage::Executable,
        }
    };

    let target = match matches.value_of("target") {
        Some("linux") => Platform::Linux,
        Some("osx") => Platform::OsX,
        _ => current_platform(),
    };

    let debug = matches.is_present("debug");
    let src_files: Vec<&str> = matches.values_of("src_files").unwrap().collect();

    let all_opts = matches.is_present("optimize");
    let opt_flags = optimize::OptimizationFlags {
        fold_constants: all_opts || matches.is_present("fold_constants"),
        eliminate_unreachable_code: all_opts || matches.is_present("eliminate_unreachable_code"),
        propagate_copies: all_opts || matches.is_present("propagate_copies"),
        eliminate_dead_stores: all_opts || matches.is_present("eliminate_dead_stores"),
    };

    driver(target, debug, stage, &src_files, &opt_flags);
}

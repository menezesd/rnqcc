use clap::{App, Arg};

mod codegen;
mod compile;
mod emit;
mod lex;
mod parse;
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
    if ext == "c" || ext == "h" {
    } else {
        panic!("Expected C source file with .c or .h extension");
    }
}

fn replace_extension(filename: &str, new_extension: &str) -> String {
    let base = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    format!("{}.{}", base, new_extension)
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

fn compile(stage: &Stage, preprocessed_src: &str, target: &Platform) -> String {
    // Compile code
    // Assume Compile::compile(stage, preprocessed_src) is handled elsewhere
    //let _ = run_command(&format!("compile {} {}", stage, preprocessed_src));
    compile::compile(stage, preprocessed_src, target);

    // Remove preprocessed source
    let cleanup_preprocessed = format!("rm {}", preprocessed_src);
    run_command(&cleanup_preprocessed);

    replace_extension(preprocessed_src, "s")
}

fn assemble_and_link(src: &str, cleanup: bool) {
    let assembly_file = replace_extension(src, "s");
    let output_file = Path::new(src)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let assemble_cmd = format!("gcc {} -o {}", assembly_file, output_file);
    run_command(&assemble_cmd);

    // Cleanup .s files
    if cleanup {
        let cleanup_cmd = format!("rm {}", assembly_file);
        run_command(&cleanup_cmd);
    }
}

fn driver(target: Platform, debug: bool, stage: Stage, src: &str) {
    let preprocessed_name = preprocess(src);
    let assembly_name = compile(&stage, &preprocessed_name, &target);

    if stage == Stage::Executable {
        assemble_and_link(&assembly_name, !debug);
    }
}

// Command-line options

fn main() {
    let matches = App::new("nqcc")
        .version("1.0")
        .author("Dean Menezes")
        .about("A not-quite-C compiler")
        .arg(
            Arg::with_name("stage")
                .short('s')
                .long("stage")
                .takes_value(true)
                .possible_values(["lex", "parse", "codegen", "s"])
                .help("Run the specified compiler stage"),
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
            Arg::with_name("src_file")
                .index(1)
                .required(true)
                .help("Input file"),
        )
        .get_matches();

    let stage = match matches.value_of("stage") {
        Some("lex") => Stage::Lex,
        Some("parse") => Stage::Parse,
        Some("codegen") => Stage::Codegen,
        Some("s") => Stage::Assembly,
        _ => Stage::Executable, // Set a default value here
    };

    let target = match matches.value_of("target") {
        Some("linux") => Platform::Linux,
        Some("osx") => Platform::OsX,
        _ => current_platform(), // You can define this function to determine default
    };

    let debug = matches.is_present("debug");
    let src_file = matches.value_of("src_file").unwrap(); // Safe unwrap due to 'required' attribute

    // Call the driver function with parsed arguments
    driver(target, debug, stage, src_file);
}

use crate::codegen;
use crate::emit;
use crate::lex;
use crate::parse;
use crate::types::*;

use std::fs::File;
use std::io::{BufRead, BufReader};

pub fn compile(stage: &Stage, src_file: &str, platform: &Platform) {
    // Read in the file
    let file = File::open(src_file).expect("File not found");
    let reader = BufReader::new(file);
    let mut source = String::new();
    for line in reader.lines() {
        source.push_str(&line.unwrap());
        source.push(' '); // Concatenate lines with a space
    }

    // Lex it
    let tokens = lex::lex(&source);

    match stage {
        Stage::Lex => (),
        Stage::Parse => {
            let _ast = parse::parse(&tokens);
            // Process ast
        }
        Stage::Codegen | Stage::Executable | Stage::Assembly => {
            let ast = parse::parse(&tokens).unwrap();
            let asm_ast = codegen::gen(ast);
            let asm_filename = src_file.trim_end_matches(".i").to_owned() + ".s";
            emit::emit(&asm_filename, &asm_ast, platform).unwrap();
        }
    }
}

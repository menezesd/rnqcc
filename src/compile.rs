use crate::codegen;
use crate::emit;
use crate::lex;
use crate::optimize;
use crate::parse;
use crate::resolve;
use crate::tacky;
use crate::types::*;

pub fn compile(stage: &Stage, src_file: &str, platform: &Platform, opt_flags: &optimize::OptimizationFlags, no_coalescing: bool) {
    // Read source file
    let source = std::fs::read_to_string(src_file)
        .unwrap_or_else(|_| panic!("Could not read file: {}", src_file));

    // Lex
    let tokens = lex::lex(&source);
    if *stage == Stage::Lex {
        return;
    }

    // Parse
    let ast = parse::parse(tokens);
    if *stage == Stage::Parse {
        return;
    }

    // Validate (resolve variables, label loops)
    let resolved_ast = resolve::resolve(ast);
    if *stage == Stage::Validate {
        return;
    }

    // Generate TACKY IR
    let mut tacky_program = tacky::generate(resolved_ast);
    if *stage == Stage::Tacky {
        return;
    }

    // Optimize TACKY IR
    optimize::optimize_program(&mut tacky_program, opt_flags);

    // Generate assembly IR and emit
    let asm_program = codegen::gen(&tacky_program, no_coalescing);
    if *stage == Stage::Codegen {
        return;
    }

    let asm_filename = src_file.trim_end_matches(".i").to_owned() + ".s";
    emit::emit(&asm_filename, &asm_program, platform).unwrap();
}

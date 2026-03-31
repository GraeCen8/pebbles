use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use inkwell::context::Context;

#[derive(Parser, Debug)]
#[command(name = "pebbles", about = "Compile Pebbles source files")]
struct Args {
    /// Input .pbl file
    input: PathBuf,
    /// Output executable path
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
    /// Keep intermediate .ll and .o files
    #[arg(long = "emit-extra-files")]
    emit_extra_files: bool,
}

fn main() {
    let args = Args::parse();
    let out_path = args.output.unwrap_or_else(|| args.input.with_extension(""));

    if let Err(e) = build_program(&args.input, &out_path, args.emit_extra_files) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn build_program(in_path: &Path, out_path: &Path, emit_extra_files: bool) -> Result<(), String> {
    let src = std::fs::read_to_string(in_path).map_err(|e| format!("read input error: {e}"))?;

    let mut lexer = pebbles::lexer::Lexer::new(&src);
    let tokens = lexer.tokenize();

    let mut parser = pebbles::parser::Parser::new(tokens);
    let ast = parser.parse().expect("parse input");

    let mut checker = pebbles::typeck::TypeChecker::new();
    checker.check(&ast).unwrap_or_else(|e| {
        eprintln!("error {}", e);
        std::process::exit(1);
    });

    let context = Context::create();
    let mut cg = pebbles::codegen::Codegen::new(&context, "pebbles");
    cg.compile(&ast).unwrap_or_else(|e| {
        eprintln!("codegen error: {e}");
        std::process::exit(1);
    });

    let ir_path = out_path.with_extension("ll");
    let obj_path = out_path.with_extension("o");
    let rt_obj_path = out_path.with_extension("rt.o");

    if emit_extra_files {
        pebbles::codegen::Codegen::write_ir(cg.module(), &ir_path)
            .map_err(|e| format!("write ir error: {e}"))?;
    }
    pebbles::codegen::Codegen::write_object(cg.module(), &obj_path)
        .map_err(|e| format!("write object error: {e}"))?;

    build_runtime(&rt_obj_path).map_err(|e| format!("runtime build error: {e}"))?;
    link_executable(&obj_path, &rt_obj_path, out_path).map_err(|e| format!("link error: {e}"))?;

    if !emit_extra_files {
        let _ = std::fs::remove_file(&obj_path);
        let _ = std::fs::remove_file(&rt_obj_path);
    }

    Ok(())
}

fn build_runtime(out_obj: &Path) -> Result<(), String> {
    let runtime = Path::new("runtime/runtime.c");
    let status = Command::new("cc")
        .args([
            "-std=c11",
            "-c",
            runtime.to_str().ok_or("bad runtime path")?,
            "-o",
            out_obj.to_str().ok_or("bad output path")?,
        ])
        .status()
        .map_err(|e| format!("failed to run cc: {e}"))?;
    if !status.success() {
        return Err("cc failed building runtime".into());
    }
    Ok(())
}

fn link_executable(obj: &Path, rt_obj: &Path, out: &Path) -> Result<(), String> {
    let status = Command::new("cc")
        .args([
            obj.to_str().ok_or("bad obj path")?,
            rt_obj.to_str().ok_or("bad rt obj path")?,
            "-no-pie",
            "-o",
            out.to_str().ok_or("bad out path")?,
        ])
        .status()
        .map_err(|e| format!("failed to run cc: {e}"))?;
    if !status.success() {
        return Err("cc failed linking".into());
    }
    Ok(())
}

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use inkwell::context::Context;

fn main() {
    let mut args = env::args().skip(1);
    let input = match args.next() {
        Some(v) => v,
        None => {
            eprintln!("usage: pebbles <input.pbl> [-o out] [--emit-llvm out.ll]");
            std::process::exit(1);
        }
    };
    let mut output = "a.out".to_string();
    let mut emit_ir: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" => {
                output = args.next().unwrap_or_else(|| "a.out".to_string());
            }
            "--emit-llvm" => {
                emit_ir = args.next();
            }
            _ => {
                eprintln!("unknown arg {arg}");
                std::process::exit(1);
            }
        }
    }

    let src = std::fs::read_to_string(&input).expect("read input");

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

    let out_path = PathBuf::from(output);
    let ir_path = emit_ir
        .map(PathBuf::from)
        .unwrap_or_else(|| out_path.with_extension("ll"));
    let obj_path = out_path.with_extension("o");
    let rt_obj_path = out_path.with_extension("rt.o");

    pebbles::codegen::Codegen::write_ir(cg.module(), &ir_path).unwrap_or_else(|e| {
        eprintln!("write ir error: {e}");
        std::process::exit(1);
    });
    pebbles::codegen::Codegen::write_object(cg.module(), &obj_path).unwrap_or_else(|e| {
        eprintln!("write object error: {e}");
        std::process::exit(1);
    });

    build_runtime(&rt_obj_path).unwrap_or_else(|e| {
        eprintln!("runtime build error: {e}");
        std::process::exit(1);
    });
    link_executable(&obj_path, &rt_obj_path, &out_path).unwrap_or_else(|e| {
        eprintln!("link error: {e}");
        std::process::exit(1);
    });
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

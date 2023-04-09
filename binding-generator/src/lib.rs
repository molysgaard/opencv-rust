// todo change dyn InputArray to impl InputArray and friends
// todo support converting pointer + size to slice of Mat and other similar objects
// todo add support for arrays in dnn::DictValue
// todo allow ergonomically combining of enum variants with |
// todo cv_utils_logging_internal_getGlobalLogTag() returns LogTag**, but Rust interprets it as LogTag*, check why it doesn't crash and fix if needed
// todo almost everything from the manual module must be connected to the binding generator, not the main crate
// todo check that FN_FaceDetector works at all (receiving InputArray, passing as callback)
// fixme vector<Mat*> get's interpreted as Vector<Mat> which should be wrong (e.g. Layer::forward and Layer::apply_halide_scheduler)
// fixme MatConstIterator::m return Mat**, is it handled correctly?
// fixme VectorOfMat::get allows mutation

// copy-pasted form python generator (may be obsolete):
// fixme returning MatAllocator (trait) by reference is bad, check knearestneighbour

#![allow(clippy::nonminimal_bool)] // pattern `!type_ref.as_vector().is_some()` used for more clarity

use std::borrow::Cow;
use std::fs::File;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::{env, fmt};
use std::path::{Path, PathBuf};

use clang::{Entity, Clang};
use dunce::canonicalize;
use once_cell::sync::Lazy;

pub use abstract_ref_wrapper::AbstractRefWrapper;
pub use class::Class;
pub use constant::Const;
pub use element::{is_opencv_path, opencv_module_from_path, DefaultElement, Element, EntityElement};
pub use entity::{EntityExt, WalkAction, WalkResult};
pub use enumeration::Enum;
use field::{Field, FieldTypeHint};
pub use func::{Func, FuncId, FunctionTypeHint};
use function::Function;
#[allow(unused)]
use generator::{dbg_clang_entity, dbg_clang_type};
pub use generator::{is_ephemeral_header, GeneratedType, Generator, GeneratorVisitor};
pub use generator_env::{ExportConfig, GeneratorEnv};
pub use iterator_ext::IteratorExt;
use memoize::{MemoizeMap, MemoizeMapExt};
use name_pool::NamePool;
use smart_ptr::SmartPtr;
pub use string_ext::{CompiledInterpolation, StrExt, StringExt};
use tuple::Tuple;
use type_ref::TypeRef;
pub use type_ref::{CppNameStyle, NameStyle};
pub use typedef::Typedef;
use vector::Vector;
pub use walker::{EntityWalker, EntityWalkerVisitor};

mod abstract_ref_wrapper;
mod class;
pub mod comment;
mod constant;
mod element;
mod entity;
mod enumeration;
mod field;
mod func;
mod function;
mod generator;
mod generator_env;
mod iterator_ext;
mod memoize;
mod name_pool;
mod renderer;
pub mod settings;
mod smart_ptr;
mod string_ext;
#[cfg(test)]
mod test;
mod tuple;
mod type_ref;
mod typedef;
mod vector;
mod walker;
pub mod writer;

use writer::RustNativeBindingWriter;
use std::io::{BufReader};

static EMIT_DEBUG: Lazy<bool> = Lazy::new(|| {
    env::var("OPENCV_BINDING_GENERATOR_EMIT_DEBUG")
        .map(|v| v == "1")
        .unwrap_or(false)
});

fn get_definition_text(entity: Entity) -> String {
    if let Some(range) = entity.get_range() {
        let loc = range.get_start().get_spelling_location();
        let mut source = File::open(loc.file.expect("Can't get file").get_path()).expect("Can't open source file");
        let start = loc.offset;
        let end = range.get_end().get_spelling_location().offset;
        let mut def_bytes = vec![0; (end - start) as usize];
        source.seek(SeekFrom::Start(u64::from(start))).expect("Cannot seek");
        source.read_exact(&mut def_bytes).expect("Can't read definition");
        String::from_utf8(def_bytes).expect("Can't parse definition")
    } else {
        unreachable!("Can't get entity range: {:#?}", entity)
    }
}

fn get_debug<'tu>(e: &(impl EntityElement<'tu> + fmt::Display)) -> String {
    if *EMIT_DEBUG {
        let loc = e
            .entity()
            .get_location()
            .expect("Can't get entity location")
            .get_file_location();

        format!(
            "// {} {}:{}",
            e,
            canonicalize(loc.file.expect("Can't get file for debug").get_path())
                .expect("Can't canonicalize path")
                .display(),
            loc.line
        )
    } else {
        "".to_string()
    }
}

fn reserved_rename(val: Cow<str>) -> Cow<str> {
    if let Some(&v) = settings::RESERVED_RENAME.get(val.as_ref()) {
        v.into()
    } else {
        val
    }
}

#[inline(always)]
fn line_reader(mut b: impl BufRead, mut cb: impl FnMut(&str) -> bool) {
    let mut line = String::with_capacity(256);
    while let Ok(bytes_read) = b.read_line(&mut line) {
        if bytes_read == 0 {
            break;
        }
        if !cb(&line) {
            break;
        }
        line.clear();
    }
}

fn get_version_header(header_dir: &Path) -> Option<PathBuf> {
    let out = header_dir.join("opencv2/core/version.hpp");
    if out.is_file() {
        Some(out)
    } else {
        let out = header_dir.join("opencv2.framework/Headers/core/version.hpp");
        if out.is_file() {
            Some(out)
        } else {
            None
        }
    }
}

pub fn get_version_from_headers(header_dir: &Path) -> Option<String> {
    let version_hpp = get_version_header(header_dir)?;
    let mut major = None;
    let mut minor = None;
    let mut revision = None;
    let mut line = String::with_capacity(256);
    let mut reader = BufReader::new(File::open(version_hpp).ok()?);
    while let Ok(bytes_read) = reader.read_line(&mut line) {
        if bytes_read == 0 {
            break;
        }
        if let Some(line) = line.strip_prefix("#define CV_VERSION_") {
            let mut parts = line.split_whitespace();
            if let (Some(ver_spec), Some(version)) = (parts.next(), parts.next()) {
                match ver_spec {
                    "MAJOR" => {
                        major = Some(version.to_string());
                    }
                    "MINOR" => {
                        minor = Some(version.to_string());
                    }
                    "REVISION" => {
                        revision = Some(version.to_string());
                    }
                    _ => {}
                }
            }
            if major.is_some() && minor.is_some() && revision.is_some() {
                break;
            }
        }
        line.clear();
    }
    if let (Some(major), Some(minor), Some(revision)) = (major, minor, revision) {
        Some(format!("{major}.{minor}.{revision}"))
    } else {
        None
    }
}


pub fn binding_generator_as_library_function(opencv_header_dir: &Path, src_cpp_dir: &Path, out_dir: &Path, module: &str, additional_include_dirs: &Vec<PathBuf>, clang: Arc<Mutex<clang::Clang>>, debug: bool) {
    assert!(opencv_header_dir.is_dir(), "opencv_header_dir must be exist and be a directory");
    assert!(src_cpp_dir.is_dir(), "src_cpp_dir must be exist and be a directory");
    assert!(out_dir.is_dir(), "out_dir must be exist and be a directory");

    let version = get_version_from_headers(&opencv_header_dir).expect("Can't find the version in the headers");
    let new_additional_include_dirs: Vec<PathBuf> = additional_include_dirs.iter().filter(|path| if path.exists() && !path.is_dir() { panic!("additional_include_dirs: {} is not a directory or does not exist", path.to_string_lossy()) } else if !path.exists() { false } else { true } ).cloned().collect();

    let bindings_writer = RustNativeBindingWriter::new(&src_cpp_dir, &out_dir, module, &version, debug);
    Generator::new(&opencv_header_dir, &new_additional_include_dirs, &src_cpp_dir, clang)
        .process_opencv_module(module, bindings_writer);
}
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn main() {
    let mut args = env::args_os().skip(1);
    let mut opencv_header_dir = args.next();
    let mut debug = false;
    if opencv_header_dir.as_ref().map_or(false, |debug| debug == "--debug") {
        debug = true;
        opencv_header_dir = args.next();
    }
    let opencv_header_dir = PathBuf::from(opencv_header_dir.expect("1st argument must be OpenCV header dir"));
    let src_cpp_dir = PathBuf::from(args.next().expect("2nd argument must be dir with custom cpp"));
    let out_dir = PathBuf::from(args.next().expect("3rd argument must be output dir"));
    let module = args.next().expect("4th argument must be module name");
    let module = module.to_str().expect("Not a valid module name");
    let additional_include_dirs = if let Some(additional_include_dirs) = args.next() {
        additional_include_dirs
            .to_str()
            .map(|s| s.split(','))
            .into_iter()
            .flatten()
            .filter(|&s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    } else {
        vec![]
    };

    let clang = Arc::new(Mutex::new(clang::Clang::new().unwrap()));

    opencv_binding_generator::binding_generator_as_library_function(&opencv_header_dir, &src_cpp_dir, &out_dir, module, &additional_include_dirs, clang, debug);
}

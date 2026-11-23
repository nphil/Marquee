#[cfg(all(target_os = "linux", target_arch = "arm"))]
#[path = "../../platform/src/storage_service/auxv.rs"]
mod auxv;
#[path = "../../platform/src/storage_service/backend.rs"]
#[cfg_attr(not(all(target_os = "linux", target_arch = "arm")), allow(dead_code))]
mod backend;
#[path = "../../platform/src/storage_service/keymanager.rs"]
#[cfg_attr(not(all(target_os = "linux", target_arch = "arm")), allow(dead_code))]
mod keymanager;
#[path = "../../platform/src/storage_service/runtime.rs"]
#[cfg_attr(not(all(target_os = "linux", target_arch = "arm")), allow(dead_code))]
mod runtime;
#[path = "../../platform/src/storage/state.rs"]
#[allow(dead_code)] // Shared engine also exposes adapter APIs unused by this executable.
mod state;
#[path = "../../platform/src/storage_service/wire.rs"]
#[cfg_attr(not(all(target_os = "linux", target_arch = "arm")), allow(dead_code))]
mod wire;

#[cfg(all(target_os = "linux", target_arch = "arm"))]
#[path = "../../platform/src/storage_service/service.rs"]
mod service;

fn main() {
    // SAM starts the service with an unlimited core size; a crash must not write a dump (which
    // can hold keys) into the install directory. Before service::run starts any thread.
    #[cfg(unix)]
    // SAFETY: setrlimit with a valid struct has no memory effects.
    unsafe {
        let zero = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        libc::setrlimit(libc::RLIMIT_CORE, &zero);
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    if service::run().is_err() {
        std::process::exit(1);
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        eprintln!("plxnative-storage requires the webOS Linux service runtime");
        std::process::exit(1);
    }
}

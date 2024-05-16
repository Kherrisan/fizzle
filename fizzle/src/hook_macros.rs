// Code from `redhook` project, available under BSD 2-Clause License

#[cfg(any(target_env = "gnu", target_os = "android"))]
pub(crate) mod ld_preload;

#[cfg(any(target_env = "gnu", target_os = "android"))]
pub(crate) use ld_preload::hook;
#[cfg(any(target_env = "gnu", target_os = "android"))]
pub(crate) use ld_preload::real;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod dyld_insert_libraries;

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) use dyld_insert_libraries::hook;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) use dyld_insert_libraries::real;

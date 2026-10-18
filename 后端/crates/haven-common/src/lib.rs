//! haven-common: 跨层共享（错误模型、时间戳、通用工具）。

pub mod error;
pub mod network;
pub mod time;
pub mod tokenizer;

pub use error::{AppError, ErrorCode, ErrorDto, ErrorKind, internal, not_found, validation};
pub use time::UtcMillis;

/// Stable local resource identity. Windows separators are equivalent; on Unix a
/// backslash is a legal filename character and must remain unchanged.
pub fn local_path_identity(path: &std::path::Path) -> String {
    let value = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    let value = value.replace('\\', "/");
    value
}

#[cfg(test)]
mod path_tests {
    #[test]
    fn local_identity_uses_native_filename_semantics() {
        let identity = super::local_path_identity(std::path::Path::new(r"root\file.txt"));
        assert_eq!(
            identity,
            if cfg!(windows) {
                "root/file.txt"
            } else {
                r"root\file.txt"
            }
        );
    }
}

//! Export file names: where an export lands and what it is called.

use std::path::{Path, PathBuf};

use application::export::ExportFormat;

/// Resolves the name typed by the user. Empty means a default name; a missing extension is added;
/// relative names go into `directory`; absolute paths are used as given.
pub fn resolve_path(directory: &Path, name: &str, format: ExportFormat) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.contains('\0') {
        return Err("The file name contains an invalid character.".into());
    }
    let name = if name.is_empty() {
        "fathom-export"
    } else {
        name
    };
    let mut path = PathBuf::from(name);
    if path.extension().is_none() {
        path.set_extension(format.extension());
    }
    if path.file_name().is_none() || name.ends_with('/') || name.ends_with('\\') {
        return Err("Enter a file name, not just a folder.".into());
    }
    Ok(if path.is_absolute() {
        path
    } else {
        directory.join(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = "/home/u/exports";

    fn resolve(name: &str, f: ExportFormat) -> Result<PathBuf, String> {
        resolve_path(Path::new(DIR), name, f)
    }

    #[test]
    fn relative_names_go_into_the_export_directory_with_an_extension() {
        assert_eq!(
            resolve("orders", ExportFormat::Csv),
            Ok(PathBuf::from("/home/u/exports/orders.csv"))
        );
        assert_eq!(
            resolve(" orders ", ExportFormat::Json),
            Ok(PathBuf::from("/home/u/exports/orders.json"))
        );
    }

    #[test]
    fn an_existing_extension_is_kept_even_if_it_differs_from_the_format() {
        assert_eq!(
            resolve("data.txt", ExportFormat::Csv),
            Ok(PathBuf::from("/home/u/exports/data.txt"))
        );
    }

    #[test]
    fn empty_means_a_default_name() {
        assert_eq!(
            resolve("", ExportFormat::Csv),
            Ok(PathBuf::from("/home/u/exports/fathom-export.csv"))
        );
        assert_eq!(
            resolve("   ", ExportFormat::Json),
            Ok(PathBuf::from("/home/u/exports/fathom-export.json"))
        );
    }

    #[test]
    fn absolute_paths_are_used_as_given() {
        assert_eq!(
            resolve("/tmp/out.csv", ExportFormat::Csv),
            Ok(PathBuf::from("/tmp/out.csv"))
        );
        assert_eq!(
            resolve("/tmp/out", ExportFormat::Json),
            Ok(PathBuf::from("/tmp/out.json"))
        );
    }

    #[test]
    fn folders_and_nul_bytes_are_rejected() {
        assert!(resolve("reports/", ExportFormat::Csv).is_err());
        assert!(resolve("a\0b", ExportFormat::Csv).is_err());
        assert!(
            resolve("..", ExportFormat::Csv)
                .map(|p| p.extension().is_some())
                .unwrap_or(true)
        );
    }
}

use std::path::{Component, Path};

pub(crate) fn validate_relative_path(field: &str, raw: &str) -> Result<(), String> {
    let relative = Path::new(raw.trim());
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir
                    | Component::CurDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "persistence.decision_evidence.{field} must be non-empty, relative, normalized, and stay under catalog_directory"
        ));
    }
    Ok(())
}

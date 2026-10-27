use serde_json::{Value, json};
use snafu::{OptionExt, Snafu, ensure};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum TitleBuildError {
    #[snafu(display("cargo {command} failed ({status}): {stderr}"))]
    CargoManifest {
        command: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[snafu(display("title provider `{package}` is not a normal, unconditional Cargo dependency"))]
    MissingProvider { package: String },
    #[snafu(display("title provider `{package}` must declare a portable registry version"))]
    VersionUnavailable { package: String },
}

pub fn bind_dependencies(definition: &mut Value, manifest: &Value) -> Result<(), TitleBuildError> {
    for dependency in definition["dependencies"]
        .as_object_mut()
        .expect("title workflow declares its providers")
        .values_mut()
    {
        let package = dependency["package"]
            .as_str()
            .expect("title provider declares its package");
        let provider = manifest["dependencies"]
            .as_array()
            .expect("Cargo package dependencies")
            .iter()
            .find(|provider| {
                provider["name"] == package
                    && provider["kind"].is_null()
                    && provider["target"].is_null()
                    && provider["optional"] == false
            })
            .context(MissingProviderSnafu { package })?;
        let version = provider["req"]
            .as_str()
            .filter(|version| *version != "*")
            .context(VersionUnavailableSnafu { package })?;
        ensure!(
            provider["source"].is_null()
                || provider["source"]
                    .as_str()
                    .is_some_and(|source| source.starts_with("registry+")),
            VersionUnavailableSnafu { package }
        );
        // Local Cargo paths cannot survive distribution of the compiled host.
        *dependency = json!({
            "package": package,
            "version": version,
            "features": provider["features"],
            "default-features": provider["uses_default_features"]
        });
    }
    Ok(())
}

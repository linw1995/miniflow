use serde_json::Value;
use std::{env, error::Error, fs, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let version = format!("={}", env::var("CARGO_PKG_VERSION")?);
    let output = env::var("OUT_DIR")?;
    for name in ["agent", "agent-turn", "tool-catalog", "tool-uses"] {
        let source = Path::new("workflows").join(format!("{name}.json"));
        println!("cargo:rerun-if-changed={}", source.display());
        let mut definition: Value = serde_json::from_slice(&fs::read(source)?)?;
        for dependency in definition["dependencies"]
            .as_object_mut()
            .expect("builtin Flow declares its node providers")
            .values_mut()
        {
            dependency["version"] = version.clone().into();
        }
        fs::write(
            Path::new(&output).join(format!("{name}.json")),
            serde_json::to_vec(&definition)?,
        )?;
    }
    Ok(())
}

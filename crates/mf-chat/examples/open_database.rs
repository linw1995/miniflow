fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("provide a database path")?;
    let database = mf_chat::ChatDatabase::open(path)?;
    println!("{}", database.path().display());
    Ok(())
}

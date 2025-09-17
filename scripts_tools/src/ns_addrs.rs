use std::io::{self, Read};

pub fn run() -> anyhow::Result<()> {
    let mut s = String::new();
    io::stdin().read_to_string(&mut s)?;
    let arr: Vec<&str> = s.split(',').filter(|a| !a.is_empty()).collect();
    println!("{}", serde_json::to_string(&arr)?);
    Ok(())
}

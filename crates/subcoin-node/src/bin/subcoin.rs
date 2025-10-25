#![allow(clippy::result_large_err)]

fn main() -> sc_cli::Result<()> {
    subcoin_node::run()?;
    Ok(())
}

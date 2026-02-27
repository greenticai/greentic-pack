#![forbid(unsafe_code)]

use std::io::Write;

use anyhow::Result;

pub fn render_text<W: Write>(output: &mut W, text: &str) -> Result<()> {
    write!(output, "{text}")?;
    Ok(())
}

pub fn render_line<W: Write>(output: &mut W, text: &str) -> Result<()> {
    writeln!(output, "{text}")?;
    Ok(())
}

pub fn render_prompt<W: Write>(output: &mut W, prompt: &str) -> Result<()> {
    write!(output, "{prompt}")?;
    output.flush()?;
    Ok(())
}

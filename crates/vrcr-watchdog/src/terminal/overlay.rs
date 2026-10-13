use std::io::{self, Write};

use super::text::{Frame, Run};

const RESET: &str = "\x1b[0m";

pub fn write_runs(out: &mut dyn Write, runs: &[Run], color: bool) -> io::Result<()> {
    if !color {
        for run in runs {
            out.write_all(run.text.as_bytes())?;
        }
        return Ok(());
    }
    for run in runs {
        out.write_all(run.color.sgr().as_bytes())?;
        out.write_all(run.text.as_bytes())?;
    }
    out.write_all(RESET.as_bytes())
}

pub fn write_frame_line(out: &mut dyn Write, frame: &Frame, color: bool) -> io::Result<()> {
    write_runs(out, &frame.runs, color)?;
    out.write_all(b"\n")
}

pub fn park_caret(out: &mut dyn Write, column: usize) -> io::Result<()> {
    out.write_all(b"\r")?;
    if column > 0 {
        write!(out, "\x1b[{column}C")?;
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct OverlayLine {
    rendered: usize,
    last: String,
    last_cursor: Option<usize>,
    visible: bool,
}

impl OverlayLine {
    pub fn rendered_len(&self) -> usize {
        self.rendered
    }

    pub fn last_rendered(&self) -> &str {
        &self.last
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn clear(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if !self.visible || self.rendered == 0 {
            return Ok(());
        }
        out.write_all(b"\r")?;
        out.write_all(" ".repeat(self.rendered).as_bytes())?;
        out.write_all(b"\r")?;
        self.rendered = 0;
        self.visible = false;
        Ok(())
    }

    pub fn render_if_changed(&mut self, out: &mut dyn Write, frame: &Frame, color: bool) -> io::Result<bool> {
        if self.visible && self.last == frame.plain && self.last_cursor == frame.cursor {
            return Ok(false);
        }
        self.clear(out)?;
        self.last.clone_from(&frame.plain);
        self.last_cursor = frame.cursor;
        if frame.plain.is_empty() {
            return Ok(false);
        }
        write_runs(out, &frame.runs, color)?;
        self.rendered = frame.width();
        self.visible = true;
        Ok(true)
    }
}

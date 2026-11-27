//! saule-pdf — a page of a PDF as a picture.
//!
//! For Preview, a Saule program: it shows pictures, and a PDF's page is
//! one once this has drawn it. Drawing is hayro's, which is all Rust and
//! brings the fourteen standard typefaces with it, so the image needs
//! nothing else to show a PDF.
//!
//!   saule-pdf info FILE              how many pages, and each one's size
//!                                    in points: `pages N`, then a line
//!                                    `page I WIDTH HEIGHT` for each
//!   saule-pdf render FILE PAGE WIDTH OUT
//!                                    page PAGE (from 1) as a PNG WIDTH
//!                                    pixels wide, written to OUT
//!
//! What cannot be done is said on standard error, and the exit is 1.

use std::path::Path;
use std::process::ExitCode;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};

/// The widest a page is drawn, in pixels: wider is memory a small machine
/// does not have, for a sharpness no screen shows.
const WIDEST: f32 = 4096.0;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.first().map(String::as_str) {
        Some("info") if args.len() == 2 => info(&args[1]),
        Some("render") if args.len() == 5 => render_page(&args[1], &args[2], &args[3], &args[4]),
        _ => Err("usage: saule-pdf info FILE | render FILE PAGE WIDTH OUT".to_string()),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn open(file: &str) -> Result<Pdf, String> {
    let bytes = std::fs::read(file).map_err(|_| "The file could not be read.".to_string())?;
    Pdf::new(bytes).map_err(|_| "The file is not a PDF document, or it is damaged.".to_string())
}

fn info(file: &str) -> Result<(), String> {
    let pdf = open(file)?;
    let pages = pdf.pages();
    let mut out = format!("pages {}\n", pages.iter().count());
    for (i, page) in pages.iter().enumerate() {
        let (width, height) = page.render_dimensions();
        out.push_str(&format!("page {} {:.2} {:.2}\n", i + 1, width, height));
    }
    print!("{out}");
    Ok(())
}

fn render_page(file: &str, page: &str, width: &str, out: &str) -> Result<(), String> {
    let number: usize = page
        .parse()
        .ok()
        .filter(|n| *n >= 1)
        .ok_or_else(|| "Which page? Give its number, from 1.".to_string())?;
    let wanted: f32 = width
        .parse()
        .ok()
        .filter(|w: &f32| w.is_finite() && *w >= 16.0)
        .ok_or_else(|| "How wide? Give a width in pixels.".to_string())?;

    let pdf = open(file)?;
    let pages = pdf.pages();
    let page = pages
        .iter()
        .nth(number - 1)
        .ok_or_else(|| "The document has no such page.".to_string())?;
    let (points, _) = page.render_dimensions();
    if points <= 0.0 {
        return Err("The page has no size.".to_string());
    }
    let scale = scale_for(points, wanted);

    let cache = RenderCache::new();
    let picture = render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let png = picture
        .into_png()
        .map_err(|_| "The page could not be made into a picture.".to_string())?;

    // Written beside where it goes and then moved there, so that what is
    // read at OUT is always a whole picture.
    let part = format!("{out}.part");
    if let Some(folder) = Path::new(out).parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    std::fs::write(&part, png).map_err(|_| "The picture could not be written.".to_string())?;
    std::fs::rename(&part, out).map_err(|_| "The picture could not be written.".to_string())
}

/// How much larger than its size in points a page `points` wide is drawn
/// to be `wanted` pixels wide, and no wider than [`WIDEST`].
fn scale_for(points: f32, wanted: f32) -> f32 {
    wanted.min(WIDEST) / points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_scaled_to_the_width_asked_for() {
        assert_eq!(scale_for(600.0, 1200.0), 2.0);
        assert_eq!(scale_for(600.0, 300.0), 0.5);
    }

    #[test]
    fn a_page_is_never_drawn_wider_than_the_widest() {
        assert_eq!(scale_for(1024.0, 100_000.0), 4.0);
    }

    #[test]
    fn what_is_not_a_pdf_is_said_to_be_none() {
        let path = std::env::temp_dir().join(format!("saule-pdf-test-{}.txt", std::process::id()));
        std::fs::write(&path, "plain text").unwrap();
        let said = info(path.to_str().unwrap());
        let _ = std::fs::remove_file(&path);
        assert_eq!(said, Err("The file is not a PDF document, or it is damaged.".to_string()));
        assert_eq!(info("/nowhere/at/all.pdf"), Err("The file could not be read.".to_string()));
    }
}

//! Multiview sessions (D17): a name plus tiles placed on a fixed 60×60 grid over the 16:9 canvas.
//! Stored on the server, unlimited in number, the same on every device.

use serde::{Deserialize, Serialize};

/// Cells per side of the canvas; at 1080p one cell is 32 × 18 px.
pub const GRID: u32 = 60;
/// Tiles per session. Each tile is decoded live, so this bounds the cost of one pipeline.
pub const MAX_TILES: usize = 9;
const MAX_NAME: usize = 80;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// Letterbox the stream inside the tile.
    #[default]
    Contain,
    /// Fill the tile, cropping the stream.
    Cover,
}

/// A stream on the grid. `x`, `y`, `w`, `h` are whole cells; equal `w` and `h` is exactly 16:9.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub channel: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Stacking order; higher is in front.
    #[serde(default)]
    pub z: u32,
    #[serde(default)]
    pub fit: Fit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub tiles: Vec<Tile>,
    /// Unix seconds.
    pub created_at: i64,
    pub updated_at: i64,
}

/// Body of `POST /api/sessions` and `PATCH /api/sessions/{id}`; missing fields stay unchanged.
#[derive(Debug, Deserialize)]
pub struct SessionInput {
    pub name: Option<String>,
    pub tiles: Option<Vec<Tile>>,
}

pub fn valid_channel(c: &str) -> bool {
    (1..=25).contains(&c.len()) && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(format!("name must be 1–{MAX_NAME} characters"));
    }
    Ok(name.to_string())
}

/// Checks the tiles and normalizes channel names to lowercase.
pub fn validate_tiles(tiles: Vec<Tile>) -> Result<Vec<Tile>, String> {
    if tiles.len() > MAX_TILES {
        return Err(format!("at most {MAX_TILES} tiles per session"));
    }
    tiles
        .into_iter()
        .enumerate()
        .map(|(i, mut t)| {
            t.channel = t.channel.trim().to_lowercase();
            if !valid_channel(&t.channel) {
                return Err(format!("tile {i}: invalid channel name"));
            }
            if t.w == 0 || t.h == 0 || t.x + t.w > GRID || t.y + t.h > GRID {
                return Err(format!("tile {i}: must lie inside the {GRID}×{GRID} grid with a size of at least one cell"));
            }
            Ok(t)
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Template {
    pub key: &'static str,
    pub name: &'static str,
    pub rects: Vec<Rect>,
}

const fn r(x: u32, y: u32, w: u32, h: u32) -> Rect {
    Rect { x, y, w, h }
}

/// Starting points for new sessions. Every rect has equal `w` and `h`, so each tile is 16:9.
pub fn templates() -> Vec<Template> {
    vec![
        Template { key: "side-by-side", name: "Side by side", rects: vec![r(0, 15, 30, 30), r(30, 15, 30, 30)] },
        Template {
            key: "grid-2x2",
            name: "2 × 2",
            rects: vec![r(0, 0, 30, 30), r(30, 0, 30, 30), r(0, 30, 30, 30), r(30, 30, 30, 30)],
        },
        Template {
            key: "big-plus-2",
            name: "1 big + 2",
            rects: vec![r(0, 10, 40, 40), r(40, 10, 20, 20), r(40, 30, 20, 20)],
        },
        Template {
            key: "big-plus-3",
            name: "1 big + 3",
            rects: vec![r(0, 7, 45, 45), r(45, 7, 15, 15), r(45, 22, 15, 15), r(45, 37, 15, 15)],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: u32, y: u32, w: u32, h: u32) -> Tile {
        Tile { channel: "Tarik".into(), x, y, w, h, z: 0, fit: Fit::Contain }
    }

    #[test]
    fn tiles_must_fit_the_grid() {
        assert!(validate_tiles(vec![tile(0, 0, 60, 60)]).is_ok());
        assert!(validate_tiles(vec![tile(30, 30, 31, 10)]).is_err());
        assert!(validate_tiles(vec![tile(0, 0, 0, 10)]).is_err());
        assert!(validate_tiles(vec![tile(0, 0, 10, 10); MAX_TILES + 1]).is_err());
    }

    #[test]
    fn channels_are_normalized_and_checked() {
        assert_eq!(validate_tiles(vec![tile(0, 0, 10, 10)]).unwrap()[0].channel, "tarik");
        let mut bad = tile(0, 0, 10, 10);
        bad.channel = "../etc".into();
        assert!(validate_tiles(vec![bad]).is_err());
    }

    #[test]
    fn templates_are_valid_and_16_by_9() {
        for t in templates() {
            let tiles = t.rects.iter().map(|r| tile(r.x, r.y, r.w, r.h)).collect();
            assert!(validate_tiles(tiles).is_ok(), "{}", t.key);
            assert!(t.rects.iter().all(|r| r.w == r.h), "{} has a non-16:9 tile", t.key);
        }
    }
}

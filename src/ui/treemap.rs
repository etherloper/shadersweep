//! Squarified treemap layout (Bruls, Huizing & van Wijk): splits a rectangle into one tile
//! per value, each tile's area proportional to its value, keeping tiles close to square.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tile {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Lays out `values` (sorted largest first, all positive) in a `width` × `height` rectangle.
/// Returns one tile per value, in the same order.
pub fn squarify(values: &[f64], width: f64, height: f64) -> Vec<Tile> {
    let total: f64 = values.iter().sum();
    if total <= 0.0 || width <= 0.0 || height <= 0.0 {
        return vec![Tile::default(); values.len()];
    }
    let scale = width * height / total;
    let areas: Vec<f64> = values.iter().map(|v| v * scale).collect();

    let mut tiles = Vec::with_capacity(areas.len());
    let (mut x, mut y, mut w, mut h) = (0.0, 0.0, width, height);
    let mut start = 0;
    while start < areas.len() {
        // Grow the row along the shorter side while that keeps its tiles squarer.
        let side = w.min(h);
        let mut end = start + 1;
        let mut best = worst_ratio(&areas[start..end], side);
        while end < areas.len() {
            let next = worst_ratio(&areas[start..=end], side);
            if next > best {
                break;
            }
            best = next;
            end += 1;
        }

        let row = &areas[start..end];
        let thickness = row.iter().sum::<f64>() / side;
        let mut offset = 0.0;
        for area in row {
            let length = area / thickness;
            if w >= h {
                tiles.push(Tile { x, y: y + offset, w: thickness, h: length });
            } else {
                tiles.push(Tile { x: x + offset, y, w: length, h: thickness });
            }
            offset += length;
        }
        if w >= h {
            x += thickness;
            w -= thickness;
        } else {
            y += thickness;
            h -= thickness;
        }
        start = end;
    }
    tiles
}

/// The most elongated aspect ratio in a row laid along a side of length `side`.
fn worst_ratio(row: &[f64], side: f64) -> f64 {
    let sum: f64 = row.iter().sum();
    let max = row.iter().cloned().fold(f64::MIN, f64::max);
    let min = row.iter().cloned().fold(f64::MAX, f64::min);
    let (side2, sum2) = (side * side, sum * sum);
    (side2 * max / sum2).max(sum2 / (side2 * min))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f64 = 1e-6;

    #[test]
    fn single_value_fills_the_rectangle() {
        assert_eq!(squarify(&[5.0], 300.0, 200.0), vec![Tile { x: 0.0, y: 0.0, w: 300.0, h: 200.0 }]);
    }

    #[test]
    fn areas_are_proportional_and_tiles_stay_inside() {
        let values = [10.0, 2.5, 1.0, 0.5, 0.5, 0.27, 0.25, 0.03];
        let (width, height) = (900.0, 500.0);
        let tiles = squarify(&values, width, height);
        let total: f64 = values.iter().sum();
        for (value, tile) in values.iter().zip(&tiles) {
            let expected = value / total * width * height;
            assert!((tile.w * tile.h - expected).abs() < 1e-3, "{tile:?}");
            assert!(tile.x >= -EPSILON && tile.y >= -EPSILON);
            assert!(tile.x + tile.w <= width + EPSILON && tile.y + tile.h <= height + EPSILON);
        }
        // Tiles don't overlap: since their areas already add up to the whole rectangle,
        // any overlap would push some tile outside it.
        for (i, a) in tiles.iter().enumerate() {
            for b in &tiles[i + 1..] {
                let overlap_w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                let overlap_h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                assert!(overlap_w <= EPSILON || overlap_h <= EPSILON, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn equal_values_make_squares() {
        let tiles = squarify(&[1.0; 4], 200.0, 200.0);
        for tile in tiles {
            assert!((tile.w - 100.0).abs() < EPSILON && (tile.h - 100.0).abs() < EPSILON);
        }
    }

    #[test]
    fn empty_input_is_handled() {
        assert!(squarify(&[], 100.0, 100.0).is_empty());
        assert_eq!(squarify(&[1.0], 0.0, 100.0), vec![Tile::default()]);
    }
}

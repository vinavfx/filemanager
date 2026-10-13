use std::ops::Range;

pub fn justified_rows(aspects: &[f32], width: f32, target: f32) -> Vec<(Range<usize>, f32)> {
    let mut rows = Vec::new();
    let mut start = 0;
    while start < aspects.len() {
        let mut end = start;
        let mut sum = 0.0;
        while end < aspects.len() {
            let next_sum = sum + aspects[end];
            let next_height = width / next_sum;
            if end > start
                && next_height < target
                && (width / sum - target).abs() < (next_height - target).abs()
            {
                break;
            }
            sum = next_sum;
            end += 1;
            if next_height <= target {
                break;
            }
        }
        let height = if end == aspects.len() {
            (width / sum).min(target)
        } else {
            width / sum
        };
        rows.push((start..end, height));
        start = end;
    }
    rows
}

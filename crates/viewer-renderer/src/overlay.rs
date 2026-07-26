use crate::DecodedImage;

pub fn draw_plan_overlay(image: &mut DecodedImage, points: &[Option<[f32; 2]>]) {
    draw_polyline(image, points, [0, 0, 0, 180], 5);
    draw_polyline(image, points, [45, 235, 165, 255], 2);
}

fn draw_polyline(
    image: &mut DecodedImage,
    points: &[Option<[f32; 2]>],
    color: [u8; 4],
    thickness: i32,
) {
    for pair in points.windows(2) {
        let [Some(start), Some(end)] = pair else {
            continue;
        };
        let Some((start, end)) = clip_line(*start, *end, image.width, image.height) else {
            continue;
        };
        let delta_x = end[0] - start[0];
        let delta_y = end[1] - start[1];
        let steps = delta_x.abs().max(delta_y.abs()).ceil().max(1.0) as u32;
        for step in 0..=steps {
            let amount = step as f32 / steps as f32;
            let x = (start[0] + delta_x * amount).round() as i32;
            let y = (start[1] + delta_y * amount).round() as i32;
            paint_square(image, x, y, thickness, color);
        }
    }
}

fn clip_line(
    start: [f32; 2],
    end: [f32; 2],
    width: u32,
    height: u32,
) -> Option<([f32; 2], [f32; 2])> {
    if width == 0 || height == 0 || start.into_iter().chain(end).any(|value| !value.is_finite()) {
        return None;
    }
    let delta = [end[0] - start[0], end[1] - start[1]];
    let bounds = [
        (-delta[0], start[0]),
        (delta[0], width.saturating_sub(1) as f32 - start[0]),
        (-delta[1], start[1]),
        (delta[1], height.saturating_sub(1) as f32 - start[1]),
    ];
    let mut minimum = 0.0_f32;
    let mut maximum = 1.0_f32;
    for (direction, distance) in bounds {
        if direction.abs() <= f32::EPSILON {
            if distance < 0.0 {
                return None;
            }
            continue;
        }
        let amount = distance / direction;
        if direction < 0.0 {
            minimum = minimum.max(amount);
        } else {
            maximum = maximum.min(amount);
        }
        if minimum > maximum {
            return None;
        }
    }
    Some((
        [start[0] + delta[0] * minimum, start[1] + delta[1] * minimum],
        [start[0] + delta[0] * maximum, start[1] + delta[1] * maximum],
    ))
}

fn paint_square(image: &mut DecodedImage, center_x: i32, center_y: i32, size: i32, color: [u8; 4]) {
    let radius = size / 2;
    for y in center_y - radius..=center_y + radius {
        for x in center_x - radius..=center_x + radius {
            if x < 0 || y < 0 || x >= image.width as i32 || y >= image.height as i32 {
                continue;
            }
            let offset = (y as usize * image.width as usize + x as usize) * 4;
            let alpha = u16::from(color[3]);
            for (channel, foreground) in color[..3].iter().copied().enumerate() {
                let background = u16::from(image.rgba[offset + channel]);
                let foreground = u16::from(foreground);
                image.rgba[offset + channel] =
                    ((foreground * alpha + background * (255 - alpha)) / 255) as u8;
            }
            image.rgba[offset + 3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_and_clips_plan_segments() {
        let mut image = DecodedImage {
            width: 20,
            height: 10,
            rgba: vec![0; 20 * 10 * 4],
        };
        draw_plan_overlay(
            &mut image,
            &[
                Some([-10.0, 5.0]),
                Some([10.0, 5.0]),
                None,
                Some([19.0, 0.0]),
            ],
        );
        let center = (5 * 20 + 5) * 4;
        assert_eq!(&image.rgba[center..center + 4], &[45, 235, 165, 255]);
        let disconnected = 19 * 4;
        assert_eq!(&image.rgba[disconnected..disconnected + 4], &[0, 0, 0, 0]);
    }
}

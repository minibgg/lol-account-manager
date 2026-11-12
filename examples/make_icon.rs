//! Рисует иконку приложения (золотой ключ в шестиграннике) и сохраняет в `assets/`:
//!
//! - `assets/icon.png` — 256×256, иконка окна (вшивается в программу через `include_bytes!`);
//! - `assets/icon.ico` — несколько размеров, иконка exe-файла и ярлыков (вшивает `build.rs`).
//!
//! Готовые файлы лежат в репозитории, так что запускать нужно, только если поменяли рисунок:
//!   cargo run --example make_icon
//!
//! Как рисуем. Каждая фигура задана «функцией расстояния» (SDF): для точки она возвращает
//! расстояние до края фигуры — отрицательное внутри, положительное снаружи. Из расстояния
//! легко получить сглаженный край: пиксель на самой границе закрашивается наполовину.
//! Так картинку любого размера можно нарисовать сразу чёткой, без масштабирования.

use std::path::Path;

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};

/// Размеры внутри .ico: Windows сама выбирает подходящий (16 — в заголовке окна,
/// 32/48 — ярлыки, 256 — крупные значки в проводнике).
const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];
const PNG_SIZE: u32 = 256;

// Цвета в духе клиента League: тёмно-синий фон и золото.
const BG_TOP: [f32; 3] = [0.051, 0.122, 0.192]; // #0D1F31
const BG_BOTTOM: [f32; 3] = [0.016, 0.039, 0.075]; // #040A13
const GOLD_LIGHT: [f32; 3] = [0.941, 0.902, 0.824]; // #F0E6D2
const GOLD: [f32; 3] = [0.784, 0.667, 0.431]; // #C8AA6E
const GOLD_DARK: [f32; 3] = [0.471, 0.353, 0.157]; // #785A28
const TEAL_GLOW: [f32; 3] = [0.039, 0.784, 0.725]; // #0AC8B9

fn main() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    if let Err(e) = write_icons(&assets) {
        eprintln!("✗ {e}");
        std::process::exit(1);
    }
}

fn write_icons(assets: &Path) -> Result<(), String> {
    std::fs::create_dir_all(assets).map_err(|e| format!("{}: {e}", assets.display()))?;

    let png = assets.join("icon.png");
    render(PNG_SIZE).save(&png).map_err(|e| format!("{}: {e}", png.display()))?;
    println!("✓ {}", png.display());

    let images: Vec<RgbaImage> = ICO_SIZES.iter().map(|&size| render(size)).collect();
    let ico = assets.join("icon.ico");
    std::fs::write(&ico, encode_ico(&images)?).map_err(|e| format!("{}: {e}", ico.display()))?;
    println!("✓ {} (размеры: {ICO_SIZES:?})", ico.display());
    Ok(())
}

/// Файл .ico: заголовок, оглавление (по записи на размер) и сами картинки.
///
/// Мелкие размеры храним как BMP, а 256×256 — как PNG: так принято в Windows,
/// и такие иконки понимают все программы (PNG внутри мелких размеров читают не все).
/// Все числа в формате — little-endian (`to_le_bytes`).
fn encode_ico(images: &[RgbaImage]) -> Result<Vec<u8>, String> {
    let entries: Vec<Vec<u8>> = images
        .iter()
        .map(|img| if img.width() >= 256 { encode_png(img) } else { Ok(encode_ico_bmp(img)) })
        .collect::<Result<_, _>>()?;

    let mut out = Vec::new();
    // ICONDIR: reserved = 0, type = 1 (иконка), количество картинок.
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());

    // ICONDIRENTRY по 16 байт; данные картинок идут сразу после оглавления.
    let mut offset = 6 + 16 * images.len();
    for (img, data) in images.iter().zip(&entries) {
        // Размер 256 записывается как 0 (в байт больше 255 не помещается).
        out.push((img.width() % 256) as u8);
        out.push((img.height() % 256) as u8);
        out.push(0); // цветов в палитре: нет палитры
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // бит на пиксель
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for data in &entries {
        out.extend_from_slice(data);
    }
    Ok(out)
}

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(png)
}

/// BMP внутри .ico: заголовок BITMAPINFOHEADER, пиксели BGRA снизу вверх и маска
/// прозрачности (1 бит на пиксель, 1 = прозрачный) — её требуют старые программы.
fn encode_ico_bmp(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = (img.width(), img.height());
    let mask_row = w.div_ceil(32) * 4; // строки маски выровнены по 4 байта
    let pixels_len = w * h * 4;
    let mut out = Vec::with_capacity((40 + pixels_len + mask_row * h) as usize);

    out.extend_from_slice(&40u32.to_le_bytes()); // размер заголовка
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&((h * 2) as i32).to_le_bytes()); // высота ×2: пиксели + маска
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // бит на пиксель
    out.extend_from_slice(&0u32.to_le_bytes()); // без сжатия
    out.extend_from_slice(&(pixels_len + mask_row * h).to_le_bytes());
    out.extend_from_slice(&[0; 16]); // разрешение и палитра не нужны

    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = img.get_pixel(x, y).0;
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    for y in (0..h).rev() {
        let mut row = vec![0u8; mask_row as usize];
        for x in 0..w {
            if img.get_pixel(x, y)[3] == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

/// Нарисовать иконку размером `size`×`size`.
fn render(size: u32) -> RgbaImage {
    // Мелкие размеры упрощаем: тонкий шестигранник там превращается в мыло,
    // поэтому оставляем только ключ и делаем его крупнее.
    let detailed = size >= 40;
    // Ширина пикселя в «нормализованных» координатах (картинка — квадрат от -1 до 1).
    let px = 2.0 / size as f32;

    RgbaImage::from_fn(size, size, |x, y| {
        // Центр пикселя → координаты от -1 до 1 (ось y направлена вниз).
        let p = [
            (x as f32 + 0.5) * px - 1.0,
            (y as f32 + 0.5) * px - 1.0,
        ];
        // Доля сверху вниз (0 — верх, 1 — низ) для вертикальных градиентов.
        let t = (p[1] + 1.0) * 0.5;

        // Слои рисуем снизу вверх, каждый — цвет + покрытие пикселя (0..1).
        let mut color = [0.0f32; 3];
        let mut alpha = 0.0f32;
        let mut paint = |rgb: [f32; 3], coverage: f32| {
            // Обычное наложение «поверх» (premultiplied не нужен: копим цвет и альфу отдельно).
            let a = coverage.clamp(0.0, 1.0);
            let out_alpha = a + alpha * (1.0 - a);
            if out_alpha > 0.0 {
                for i in 0..3 {
                    color[i] = (rgb[i] * a + color[i] * alpha * (1.0 - a)) / out_alpha;
                }
            }
            alpha = out_alpha;
        };

        // 1. Фон — скруглённый квадрат с градиентом и тонкой золотой рамкой.
        let bg = sd_round_box(p, 0.94, 0.26);
        paint(mix(BG_TOP, BG_BOTTOM, t), cover(bg, px));
        let frame_width = (0.018f32).max(px * 0.6);
        // Кольцо шириной 2·frame_width прямо у края, внутри фона.
        paint(GOLD_DARK, cover((bg + frame_width).abs() - frame_width, px) * 0.9);

        let key_scale = if detailed { 1.0 } else { 1.3 };
        if detailed {
            // 2. Шестигранник: лёгкое бирюзовое свечение внутри и золотой контур.
            let hex = sd_hexagon_pointy(p, 0.70);
            paint(TEAL_GLOW, cover(hex, px) * 0.10 * (1.0 - t));
            paint(gold(t), cover(hex.abs() - 0.04, px));
        }

        // 3. Ключ: сначала мягкая тень, потом сам ключ.
        let shadow = sd_key([p[0] - 0.03, p[1] - 0.04], key_scale);
        paint([0.0, 0.0, 0.0], cover(shadow - 0.02, px * 4.0) * 0.45);
        let key = sd_key(p, key_scale);
        paint(gold(t * 0.8), cover(key, px));
        // Тонкая тёмная обводка, чтобы ключ не сливался с контуром шестигранника.
        paint(GOLD_DARK, cover(key.abs() - px * 0.5, px) * 0.8);

        Rgba([
            to_u8(color[0]),
            to_u8(color[1]),
            to_u8(color[2]),
            to_u8(alpha),
        ])
    })
}

/// Ключ по диагонали: кольцо-головка слева сверху, стержень с бородкой вправо вниз.
fn sd_key(p: [f32; 2], scale: f32) -> f32 {
    let p = [p[0] / scale, p[1] / scale];
    // Поворот на 45°: u — вдоль стержня, v — поперёк (бородка в сторону +v).
    let r = std::f32::consts::FRAC_1_SQRT_2;
    let u = (p[0] + p[1]) * r + 0.05;
    let v = (p[1] - p[0]) * r;
    let q = [u, v];

    let head = sd_circle(q, [-0.30, 0.0], 0.23);
    let hole = sd_circle(q, [-0.30, 0.0], 0.095);
    let ring = head.max(-hole); // вычитание: головка без отверстия
    let shaft = sd_box(q, [0.14, 0.0], [0.30, 0.065]);
    let tooth1 = sd_box(q, [0.225, 0.11], [0.045, 0.09]);
    let tooth2 = sd_box(q, [0.385, 0.09], [0.045, 0.07]);
    // Объединение фигур — минимум расстояний; результат обратно в масштаб картинки.
    ring.min(shaft).min(tooth1).min(tooth2) * scale
}

/// Золотой градиент: светлое сверху → тёмное снизу.
fn gold(t: f32) -> [f32; 3] {
    if t < 0.5 {
        mix(GOLD_LIGHT, GOLD, t * 2.0)
    } else {
        mix(GOLD, GOLD_DARK, (t - 0.5) * 2.0)
    }
}

/// Доля пикселя внутри фигуры: 1 — целиком внутри, 0 — снаружи, на краю — плавно.
fn cover(distance: f32, px: f32) -> f32 {
    (0.5 - distance / px).clamp(0.0, 1.0)
}

fn sd_circle(p: [f32; 2], center: [f32; 2], radius: f32) -> f32 {
    (p[0] - center[0]).hypot(p[1] - center[1]) - radius
}

/// Прямоугольник с центром `center` и половинами сторон `half`.
fn sd_box(p: [f32; 2], center: [f32; 2], half: [f32; 2]) -> f32 {
    let dx = (p[0] - center[0]).abs() - half[0];
    let dy = (p[1] - center[1]).abs() - half[1];
    dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0)
}

/// Квадрат с центром в нуле, половиной стороны `half` и радиусом скругления `radius`.
fn sd_round_box(p: [f32; 2], half: f32, radius: f32) -> f32 {
    sd_box(p, [0.0, 0.0], [half - radius, half - radius]) - radius
}

/// Правильный шестиугольник «вершиной вверх»; `apothem` — расстояние от центра до стороны.
fn sd_hexagon_pointy(p: [f32; 2], apothem: f32) -> f32 {
    // Формула Иниго Килеса (iquilezles.org) для шестиугольника с плоским верхом,
    // x и y переставлены, чтобы вершина смотрела вверх.
    let (kx, ky, kz) = (-0.866_025_4f32, 0.5f32, 0.577_350_3f32);
    let (mut x, mut y) = (p[1].abs(), p[0].abs());
    let d = 2.0 * (kx * x + ky * y).min(0.0);
    x -= d * kx;
    y -= d * ky;
    let cx = x - x.clamp(-kz * apothem, kz * apothem);
    let cy = y - apothem;
    cx.hypot(cy) * cy.signum()
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_have_expected_signs() {
        assert!(sd_circle([0.0, 0.0], [0.0, 0.0], 0.5) < 0.0);
        assert!((sd_box([1.0, 0.0], [0.0, 0.0], [0.5, 0.5]) - 0.5).abs() < 1e-6);
        assert!(sd_hexagon_pointy([0.0, 0.0], 0.7) < 0.0);
        assert!(sd_hexagon_pointy([0.0, 0.9], 0.7) > 0.0, "выше вершины — снаружи");
        assert!(sd_hexagon_pointy([0.75, 0.0], 0.7) > 0.0, "правее стороны — снаружи");
    }

    #[test]
    fn icon_is_opaque_inside_and_transparent_in_corners() {
        let icon = render(64);
        assert_eq!(icon.get_pixel(32, 32)[3], 255);
        assert_eq!(icon.get_pixel(0, 0)[3], 0, "углы скруглены — прозрачные");
    }

    #[test]
    fn ico_has_directory_bmp_and_png_frames() {
        let ico = encode_ico(&[render(16), render(256)]).unwrap();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0], "заголовок: иконка из 2 картинок");
        assert_eq!((ico[6], ico[6 + 16]), (16, 0), "256 записывается как 0");
        // Смещение данных — последние 4 байта записи оглавления.
        let offset = |entry: usize| {
            let at = 6 + 16 * entry + 12;
            u32::from_le_bytes(ico[at..at + 4].try_into().unwrap()) as usize
        };
        assert_eq!(offset(0), 6 + 16 * 2);
        assert_eq!(&ico[offset(0)..offset(0) + 4], &40u32.to_le_bytes(), "16×16 — BMP");
        assert!(ico[offset(1)..].starts_with(b"\x89PNG"), "256×256 — PNG");
        // BMP 16×16: заголовок 40 + пиксели 16·16·4 + маска 16 строк по 4 байта.
        assert_eq!(offset(1) - offset(0), 40 + 1024 + 64);
    }
}

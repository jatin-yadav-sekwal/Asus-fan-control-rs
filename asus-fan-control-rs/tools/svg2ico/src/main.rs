//! Render a square SVG logo into a multi-resolution Windows `.ico`.
//!
//! Usage: svg2ico <input.svg> <output.ico> [png:WxH=<path>]

use std::fs;
use std::path::PathBuf;

const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: svg2ico <input.svg> <output.ico> [png:WxH=<path>]");
        std::process::exit(2);
    }

    let svg_path = PathBuf::from(&args[0]);
    let ico_path = PathBuf::from(&args[1]);

    let svg_text = fs::read_to_string(&svg_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", svg_path.display(), e));

    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(&svg_text, &options)
        .unwrap_or_else(|e| panic!("cannot parse {}: {}", svg_path.display(), e));

    let natural = tree.size().width().max(1.0);

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in SIZES {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
            .unwrap_or_else(|| panic!("{}x{} pixmap unsupported", size, size));
        let scale = size as f32 / natural;
        let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let image = ico::IconImage::from_rgba_data(size, size, pixmap.take());
        let entry = ico::IconDirEntry::encode(&image).expect("ICO entry encode");
        dir.add_entry(entry);
        println!("  + {}x{}", size, size);
    }

    if let Some(parent) = ico_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut out = fs::File::create(&ico_path)
        .unwrap_or_else(|e| panic!("cannot create {}: {}", ico_path.display(), e));
    dir.write(&mut out)
        .unwrap_or_else(|e| panic!("cannot write {}: {}", ico_path.display(), e));
    println!("wrote {}", ico_path.display());

    for spec in args.iter().skip(2) {
        if let Some(rest) = spec.strip_prefix("png:") {
            if let Some((dims, path)) = rest.split_once('=') {
                let (w, h): (u32, u32) = match dims.split_once('x') {
                    Some((a, b)) => (a.parse().unwrap(), b.parse().unwrap()),
                    None => (1280, 640),
                };
                write_png(&tree, w, h, PathBuf::from(path));
            }
        }
    }
}

fn write_png(tree: &resvg::usvg::Tree, w: u32, h: u32, path: PathBuf) {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).expect("pixmap");
    let sw = w as f32 / tree.size().width();
    let sh = h as f32 / tree.size().height();
    let scale = sw.min(sh);
    let tx = (w as f32 - tree.size().width() * scale) / 2.0;
    let ty = (h as f32 - tree.size().height() * scale) / 2.0;
    let transform = resvg::tiny_skia::Transform::from_translate(tx, ty).post_scale(scale, scale);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    pixmap
        .save_png(&path)
        .unwrap_or_else(|e| panic!("cannot write {}: {}", path.display(), e));
    println!("wrote {}", path.display());
}

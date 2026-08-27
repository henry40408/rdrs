use std::io::BufWriter;
use std::path::Path;
use std::process::Command;

fn main() {
    // Re-run build script when git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    // Re-run when static assets are added/removed/modified
    println!("cargo:rerun-if-changed=static");

    let git_version = get_git_version();
    println!("cargo:rustc-env=GIT_VERSION={git_version}");

    // Generate favicon files
    println!("cargo:rerun-if-changed=favicon.svg");
    generate_favicons();
}

fn get_git_version() -> String {
    // First, check if GIT_VERSION is set via environment variable
    // This is used for Docker builds where .git directory is not available
    if let Ok(version) = std::env::var("GIT_VERSION")
        && !version.is_empty()
        && version != "dev"
    {
        return version;
    }

    // git describe --tags --always --dirty
    // --tags: Use both annotated and lightweight tags
    // --always: Fall back to commit hash when no tags exist
    // --dirty: Append -dirty suffix when there are uncommitted changes
    Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "dev".to_string(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        )
}

fn generate_favicons() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir);

    let svg_data = std::fs::read("favicon.svg").expect("Failed to read favicon.svg");

    // Parse SVG
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(&svg_data, &options).expect("Failed to parse SVG");

    // Opaque background for the home-screen icons. iOS does not support
    // transparency on home-screen icons (transparent pixels render as black) and
    // applies its own rounded-corner mask; Android's adaptive-icon mask has the
    // same problem. We therefore flatten those onto a solid background matching
    // the SVG's outer frame color (#1A0E08) so the corners stay full-bleed
    // instead of transparent/self-rounded.
    let opaque_bg = resvg::tiny_skia::Color::from_rgba8(0x1A, 0x0E, 0x08, 0xFF);

    // Generate PNGs in various sizes. Favicons keep a transparent background and
    // fill the canvas; the home-screen icons are rendered opaque and full-bleed.
    //
    // `maskable-icon-512.png` is the exception. A `purpose: "maskable"` icon is
    // cropped to whatever shape the launcher wants — a circle, on most Android
    // versions — and only the central 80% (the spec's "safe zone") is guaranteed
    // to survive. Rendering the artwork at 0.8 keeps the mark inside it, with
    // the same #1A0E08 the SVG's own frame uses bleeding to the edges so the
    // crop stays seamless. The `any` icons stay at 1.0 because those are shown
    // uncropped, where the same margin would just letterbox the icon.
    let sizes = [
        (16, "favicon-16x16.png", None, 1.0),
        (32, "favicon-32x32.png", None, 1.0),
        (180, "apple-touch-icon.png", Some(opaque_bg), 1.0),
        (192, "icon-192.png", Some(opaque_bg), 1.0),
        (512, "icon-512.png", Some(opaque_bg), 1.0),
        (512, "maskable-icon-512.png", Some(opaque_bg), 0.8),
    ];

    for (size, filename, background, content_scale) in sizes {
        let png_data = render_svg_to_png(&tree, size, background, content_scale);
        let path = out_path.join(filename);
        std::fs::write(&path, &png_data).unwrap_or_else(|_| panic!("Failed to write {filename}"));
    }

    // Generate ICO file (contains 16x16 and 32x32)
    generate_ico(&tree, out_path);

    // Copy original SVG to OUT_DIR
    std::fs::copy("favicon.svg", out_path.join("favicon.svg")).expect("Failed to copy favicon.svg");
}

/// Render `tree` into a `size`x`size` PNG.
///
/// `content_scale` is the fraction of the canvas the artwork covers, centred:
/// 1.0 fills it, 0.8 leaves the maskable safe-zone margin described above.
fn render_svg_to_png(
    tree: &resvg::usvg::Tree,
    size: u32,
    background: Option<resvg::tiny_skia::Color>,
    content_scale: f32,
) -> Vec<u8> {
    let tree_size = tree.size();
    let scale = (size as f32 * content_scale) / tree_size.width().max(tree_size.height());

    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).unwrap();

    // Fill with an opaque background before rendering so transparent areas of
    // the SVG (e.g. its rounded corners) become solid instead of transparent.
    if let Some(color) = background {
        pixmap.fill(color);
    }

    // Calculate centering offset
    let scaled_w = tree_size.width() * scale;
    let scaled_h = tree_size.height() * scale;
    let offset_x = (size as f32 - scaled_w) / 2.0;
    let offset_y = (size as f32 - scaled_h) / 2.0;

    let transform =
        resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(offset_x, offset_y);

    resvg::render(tree, transform, &mut pixmap.as_mut());
    pixmap.encode_png().unwrap()
}

fn generate_ico(tree: &resvg::usvg::Tree, out_path: &Path) {
    let ico_path = out_path.join("favicon.ico");
    let file = std::fs::File::create(&ico_path).expect("Failed to create favicon.ico");
    let writer = BufWriter::new(file);

    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);

    for size in [16u32, 32u32] {
        let png_data = render_svg_to_png(tree, size, None, 1.0);
        let img = image::load_from_memory(&png_data).expect("Failed to load PNG");
        let rgba = img.to_rgba8();
        let ico_image = ico::IconImage::from_rgba_data(size, size, rgba.into_raw());
        icon_dir.add_entry(ico::IconDirEntry::encode(&ico_image).unwrap());
    }

    icon_dir.write(writer).expect("Failed to write ICO");
}

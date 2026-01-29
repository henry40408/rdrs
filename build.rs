use std::io::BufWriter;
use std::path::Path;
use std::process::Command;

fn main() {
    // 當 git HEAD 變更時重新執行 build script
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let git_version = get_git_version();
    println!("cargo:rustc-env=GIT_VERSION={}", git_version);

    // 生成 favicon 檔案
    println!("cargo:rerun-if-changed=favicon.svg");
    generate_favicons();
}

fn get_git_version() -> String {
    // git describe --tags --always --dirty
    // --tags: 使用 annotated 和 lightweight tags
    // --always: 沒有 tag 時 fallback 到 commit hash
    // --dirty: 有未提交的變更時加上 -dirty 後綴
    Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "dev".to_string())
}

fn generate_favicons() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir);

    // 讀取 SVG 檔案
    let svg_data = std::fs::read("favicon.svg").expect("Failed to read favicon.svg");

    // 解析 SVG
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(&svg_data, &options).expect("Failed to parse SVG");

    // 生成各種尺寸的 PNG
    let sizes = [
        (16, "favicon-16x16.png"),
        (32, "favicon-32x32.png"),
        (180, "apple-touch-icon.png"),
    ];

    for (size, filename) in sizes {
        let png_data = render_svg_to_png(&tree, size);
        let path = out_path.join(filename);
        std::fs::write(&path, &png_data).unwrap_or_else(|_| panic!("Failed to write {}", filename));
    }

    // 生成 ICO 檔案（包含 16x16 和 32x32）
    generate_ico(&tree, out_path);

    // 複製原始 SVG 到 OUT_DIR
    std::fs::copy("favicon.svg", out_path.join("favicon.svg")).expect("Failed to copy favicon.svg");
}

fn render_svg_to_png(tree: &resvg::usvg::Tree, size: u32) -> Vec<u8> {
    let tree_size = tree.size();
    let scale = size as f32 / tree_size.width().max(tree_size.height());

    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).unwrap();

    // 計算置中偏移
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

    // 新增 16x16 和 32x32 圖片
    for size in [16u32, 32u32] {
        let png_data = render_svg_to_png(tree, size);
        let img = image::load_from_memory(&png_data).expect("Failed to load PNG");
        let rgba = img.to_rgba8();
        let ico_image = ico::IconImage::from_rgba_data(size, size, rgba.into_raw());
        icon_dir.add_entry(ico::IconDirEntry::encode(&ico_image).unwrap());
    }

    icon_dir.write(writer).expect("Failed to write ICO");
}

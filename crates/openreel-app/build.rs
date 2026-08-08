use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"));
    let icon_path = out_dir.join("OpenReel.ico");
    let resource_script = out_dir.join("OpenReel.rc");
    let compiled_resource = out_dir.join("OpenReel.res");
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo must set CARGO_PKG_VERSION");

    write_icon(&icon_path).expect("failed to generate the OpenReel icon");
    write_resource_script(&resource_script, &icon_path, &version)
        .expect("failed to generate the OpenReel Windows resource script");

    let output = Command::new("rc.exe")
        .arg("/nologo")
        .arg("/fo")
        .arg(&compiled_resource)
        .arg(&resource_script)
        .output()
        .expect(
            "rc.exe was not found; build from a Visual Studio developer shell or run scripts/setup-ffmpeg.ps1 first",
        );

    if !output.status.success() {
        panic!(
            "rc.exe failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    println!(
        "cargo:rustc-link-arg-bin=openreel-app={}",
        compiled_resource.display()
    );
}

fn write_resource_script(path: &Path, icon_path: &Path, version: &str) -> std::io::Result<()> {
    let numbers = numeric_version(version);
    let icon_path = icon_path.to_string_lossy().replace('\\', "/");
    let script = format!(
        r#"1 ICON "{icon_path}"

1 VERSIONINFO
 FILEVERSION {major},{minor},{patch},{build}
 PRODUCTVERSION {major},{minor},{patch},{build}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "OpenReel contributors\0"
            VALUE "FileDescription", "OpenReel video editor\0"
            VALUE "FileVersion", "{version}\0"
            VALUE "InternalName", "OpenReel\0"
            VALUE "LegalCopyright", "Copyright OpenReel contributors\0"
            VALUE "LegalTrademarks", "Licensed under GPL-3.0-only\0"
            VALUE "OriginalFilename", "OpenReel.exe\0"
            VALUE "ProductName", "OpenReel\0"
            VALUE "ProductVersion", "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#,
        major = numbers[0],
        minor = numbers[1],
        patch = numbers[2],
        build = numbers[3],
    );

    fs::write(path, script)
}

fn numeric_version(version: &str) -> [u16; 4] {
    let numeric = version.split_once('-').map_or(version, |(value, _)| value);
    let mut result = [0_u16; 4];

    for (slot, component) in result.iter_mut().zip(numeric.split('.')) {
        *slot = component
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("version component is not a 16-bit integer: {component}"));
    }

    result
}

fn write_icon(path: &Path) -> std::io::Result<()> {
    let sizes = [16_u32, 32, 48, 256];
    let images = sizes.map(render_icon_image);
    let directory_size = 6 + (16 * images.len());
    let total_size = directory_size + images.iter().map(Vec::len).sum::<usize>();
    let mut icon = Vec::with_capacity(total_size);

    push_u16(&mut icon, 0);
    push_u16(&mut icon, 1);
    push_u16(
        &mut icon,
        u16::try_from(images.len()).expect("icon image count fits in u16"),
    );

    let mut offset = u32::try_from(directory_size).expect("icon directory size fits in u32");
    for (size, image) in sizes.iter().zip(images.iter()) {
        icon.push(if *size == 256 { 0 } else { *size as u8 });
        icon.push(if *size == 256 { 0 } else { *size as u8 });
        icon.push(0);
        icon.push(0);
        push_u16(&mut icon, 1);
        push_u16(&mut icon, 32);
        push_u32(
            &mut icon,
            u32::try_from(image.len()).expect("icon image size fits in u32"),
        );
        push_u32(&mut icon, offset);
        offset += u32::try_from(image.len()).expect("icon image size fits in u32");
    }

    for image in images {
        icon.extend_from_slice(&image);
    }

    fs::write(path, icon)
}

fn render_icon_image(size: u32) -> Vec<u8> {
    let pixel_bytes = size * size * 4;
    let mask_row_bytes = size.div_ceil(32) * 4;
    let mask_bytes = mask_row_bytes * size;
    let mut image = Vec::with_capacity((40 + pixel_bytes + mask_bytes) as usize);

    push_u32(&mut image, 40);
    push_u32(&mut image, size);
    push_u32(&mut image, size * 2);
    push_u16(&mut image, 1);
    push_u16(&mut image, 32);
    push_u32(&mut image, 0);
    push_u32(&mut image, pixel_bytes);
    push_u32(&mut image, 0);
    push_u32(&mut image, 0);
    push_u32(&mut image, 0);
    push_u32(&mut image, 0);

    let side = size as f32;
    let corner_radius = side * 0.22;
    let half = side / 2.0;

    for row in (0..size).rev() {
        for column in 0..size {
            let x = column as f32 + 0.5;
            let y = row as f32 + 0.5;
            let corner_x = (x - half).abs() - (half - corner_radius);
            let corner_y = (y - half).abs() - (half - corner_radius);
            let outside_x = corner_x.max(0.0);
            let outside_y = corner_y.max(0.0);
            let corner_distance = outside_x.hypot(outside_y);
            let alpha = ((corner_radius + 0.5 - corner_distance) * 255.0).clamp(0.0, 255.0);

            if alpha == 0.0 {
                image.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }

            let vertical = y / side;
            let mut red = 18.0 + (40.0 * vertical);
            let mut green = 28.0 + (34.0 * vertical);
            let mut blue = 56.0 + (70.0 * vertical);

            let center_x = side * 0.46;
            let center_y = side * 0.50;
            let distance = (x - center_x).hypot(y - center_y);
            let ring_radius = side * 0.285;
            let ring_width = (side * 0.055).max(1.0);
            if (distance - ring_radius).abs() <= ring_width {
                red = 109.0;
                green = 226.0;
                blue = 255.0;
            }

            let triangle = point_in_triangle(
                (x, y),
                (side * 0.39, side * 0.33),
                (side * 0.39, side * 0.67),
                (side * 0.68, side * 0.50),
            );
            if triangle {
                red = 248.0;
                green = 250.0;
                blue = 252.0;
            }

            image.push(blue as u8);
            image.push(green as u8);
            image.push(red as u8);
            image.push(alpha as u8);
        }
    }

    image.resize((40 + pixel_bytes + mask_bytes) as usize, 0);
    image
}

fn point_in_triangle(point: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let sign = |p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| {
        (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1)
    };
    let first = sign(point, a, b);
    let second = sign(point, b, c);
    let third = sign(point, c, a);
    let has_negative = first < 0.0 || second < 0.0 || third < 0.0;
    let has_positive = first > 0.0 || second > 0.0 || third > 0.0;

    !(has_negative && has_positive)
}

fn push_u16(target: &mut Vec<u8>, value: u16) {
    target.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(target: &mut Vec<u8>, value: u32) {
    target.extend_from_slice(&value.to_le_bytes());
}

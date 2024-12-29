use std::process::Command;

const ASM_FILE: &str = "asm/context.S";
const O_FILE: &str = "asm/context.o";
const LIB_FILE: &str = "asm/libcontext.a";

// build.rs という名前のファイルには特別な意味がある
// プロジェクトルートに置いておくと、ビルド前に実行される
// cargo: で始まる命令は Cargo が特別な意味を持って解釈するらしい

fn main() {
    Command::new("cc")
        .args([ASM_FILE, "-c", "-fPIC", "-ggdb", "-o"])
        .arg(O_FILE)
        .status()
        .unwrap();
    Command::new("ar")
        .args(["crus", LIB_FILE, O_FILE])
        .status()
        .unwrap();

    println!("cargo:rustc-link-search=native=asm"); // asm をライブラリ検索パスに追加
    println!("cargo:rustc-link-lib=static=context"); // libcontext.o という静的ライブラリをリンク,  prefix の lib と .o という拡張子を除いて指定
    println!("cargo:return-if-changed=asm/context.S"); // asm/context.S というファイルに保存
}

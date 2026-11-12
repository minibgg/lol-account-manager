//! Сборочный скрипт: Cargo запускает его перед компиляцией программы.
//!
//! На Windows вшивает в `lol-account-manager.exe` иконку (`assets/icon.ico`) и название
//! программы: их показывают проводник, ярлыки, панель задач и диспетчер задач.
//! Нужен компилятор ресурсов rc.exe — он входит в Windows SDK (ставится вместе
//! с Visual Studio Build Tools). Если его нет, программа всё равно соберётся,
//! просто со стандартной иконкой.

fn main() {
    // Пересобирать ресурсы, только если поменялась иконка или сам скрипт.
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    // CARGO_CFG_TARGET_OS — система, ПОД которую собираем (не та, на которой собираем).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/icon.ico")
        // FileDescription Windows показывает как имя программы (например, в диспетчере задач).
        .set("FileDescription", "LoL Account Manager")
        .set("ProductName", "LoL Account Manager");
    // compile_for — только для программы, а не для примеров и тестов.
    if let Err(e) = resource.compile_for("lol-account-manager") {
        println!("cargo:warning=иконка не вшита в exe (нужен rc.exe из Windows SDK): {e}");
    }
}

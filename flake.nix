{
  description = "invisible-ai — screen- and meeting-aware AI overlay for Linux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # Tauri v2 (WebKitGTK) runtime + build deps
        tauriLibs = with pkgs; [
          at-spi2-atk
          atkmm
          cairo
          gdk-pixbuf
          glib
          gtk3
          harfbuzz
          librsvg
          libsoup_3
          pango
          webkitgtk_4_1
          openssl
        ];

        # Platform backends (M1–M5)
        backendLibs = with pkgs; [
          gtk-layer-shell # Wayland overlay
          libx11
          libxext # XShm capture
          libxfixes # click-through input shape
          pipewire # audio + portal screencast streams
          dbus # xdg-desktop-portal
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            pkg-config
            gobject-introspection
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            cargo-tauri
            llvmPackages.libclang # bindgen (pipewire, whisper-rs)
            cmake # whisper.cpp build
          ];

          buildInputs = tauriLibs ++ backendLibs;

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (tauriLibs ++ backendLibs);
          # WebKitGTK renders blank on some GPU/driver combos without this
          WEBKIT_DISABLE_DMABUF_RENDERER = "1";
        };
      });
}

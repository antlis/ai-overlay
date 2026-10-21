{
  description = "ai-overlay — screen- and meeting-aware AI overlay for Linux";

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
          libgbm # xcap: wlroots screencopy buffers
          libayatana-appindicator # tray icon (falls back to XEmbed trays like i3bar)
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
            rustPlatform.bindgenHook # libclang + libc headers for bindgen (pipewire, via xcap)
            cmake # whisper.cpp build
            # Programs the app runs (also recorded for hotkey starts, see app/build.rs).
            whisper-cpp # whisper-server: voice
            piper-tts # piper: spoken answers
            grim # screenshots on Wayland
            wl-clipboard # copy on Wayland
            xclip # copy on X11
          ];

          buildInputs = tauriLibs ++ backendLibs;

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (tauriLibs ++ backendLibs);
          # Don't set WEBKIT_DISABLE_DMABUF_RENDERER: with the fallback renderer, partial
          # repaints of the transparent overlay blank out everything else.
        };
      });
}

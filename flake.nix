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

        # Programs the app runs: whisper-server (voice), piper (spoken answers),
        # pw-record / pw-play (audio), grim (screenshots on Wayland), wl-copy / xclip (copy buttons).
        runtimeBins = with pkgs; [
          whisper-cpp
          piper-tts
          pipewire
          grim
          wl-clipboard
          xclip
        ];
      in
      {
        # CI builds this and pushes it to Cachix (.github/workflows/nix-cache.yml). The cached
        # build only matches if the consumer doesn't override this flake's nixpkgs input.
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "ai-overlay";
          version = (pkgs.lib.importTOML ./Cargo.toml).workspace.package.version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = with pkgs; [
            pkg-config
            gobject-introspection
            cargo-tauri
            cmake
            makeWrapper
            rustPlatform.bindgenHook
          ];
          buildInputs = tauriLibs ++ backendLibs;
          RUSTFLAGS = "-C link-arg=-Wl,-z,stack-size=8388608";
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (tauriLibs ++ backendLibs);
          postInstall = ''
            wrapProgram $out/bin/ai-overlay --prefix PATH : ${pkgs.lib.makeBinPath runtimeBins}
          '';
        };

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

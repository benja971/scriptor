{
  description = "scriptor dev shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      playwrightBrowsers = pkgs.playwright-driver.selectBrowsers {
        withWebkit = false;
        withChromiumHeadlessShell = true;
      };
      pageRenderer = pkgs.writeShellApplication {
        name = "scriptor-page-renderer";
        runtimeInputs = [ pkgs.nodejs pkgs.playwright-test playwrightBrowsers ];
        text = ''
          export NODE_PATH="${pkgs.playwright-test}/lib/node_modules"
          export PLAYWRIGHT_BROWSERS_PATH="${playwrightBrowsers}"
          exec node ${./scripts/page-renderer.mjs} "$@"
        '';
      };
      pageRendererTest = pkgs.writeShellApplication {
        name = "scriptor-page-renderer-test";
        runtimeInputs = [ pkgs.nodejs pkgs.playwright-test playwrightBrowsers ];
        text = ''
          export NODE_PATH="${pkgs.playwright-test}/lib/node_modules"
          export PLAYWRIGHT_BROWSERS_PATH="${playwrightBrowsers}"
          export SCRIPTOR_PAGE_RENDERER_MODULE="${./scripts/page-renderer.mjs}"
          exec node ${./scripts/page-renderer.test.mjs}
        '';
      };
    in {
      devShells.${system}.default = pkgs.mkShell {
        buildInputs = [
          pkgs.cargo
          pkgs.rustc
          pkgs.rust-analyzer
          pkgs.clippy
          pkgs.rustfmt

          pkgs.ffmpeg
          pkgs.whisper-cpp
          pkgs.yt-dlp
          pkgs.libnotify
          pageRenderer
          pageRendererTest
        ];
      };
    };
}

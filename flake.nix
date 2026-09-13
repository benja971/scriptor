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
      webScripts = pkgs.runCommand "scriptor-web-scripts" { } ''
        mkdir -p "$out"
        cp ${./scripts/page-renderer.mjs} "$out/page-renderer.mjs"
        cp ${./scripts/binary-acquirer.mjs} "$out/binary-acquirer.mjs"
      '';
      pageRenderer = pkgs.writeShellApplication {
        name = "scriptor-page-renderer";
        runtimeInputs = [ pkgs.nodejs pkgs.curl pkgs.playwright-test playwrightBrowsers ];
        text = ''
          export NODE_PATH="${pkgs.playwright-test}/lib/node_modules"
          export PLAYWRIGHT_BROWSERS_PATH="${playwrightBrowsers}"
          exec node ${webScripts}/page-renderer.mjs "$@"
        '';
      };
      pageRendererTest = pkgs.writeShellApplication {
        name = "scriptor-page-renderer-test";
        runtimeInputs = [ pkgs.nodejs pkgs.curl pkgs.playwright-test playwrightBrowsers ];
        text = ''
          export NODE_PATH="${pkgs.playwright-test}/lib/node_modules"
          export PLAYWRIGHT_BROWSERS_PATH="${playwrightBrowsers}"
          export SCRIPTOR_PAGE_RENDERER_MODULE="${webScripts}/page-renderer.mjs"
          export SCRIPTOR_BINARY_ACQUIRER_MODULE="${webScripts}/binary-acquirer.mjs"
          exec node ${./scripts/page-renderer.test.mjs}
        '';
      };
      binaryAcquirer = pkgs.writeShellApplication {
        name = "scriptor-binary-acquirer";
        runtimeInputs = [ pkgs.nodejs pkgs.curl ];
        text = ''
          exec node ${webScripts}/binary-acquirer.mjs "$@"
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
          binaryAcquirer
        ];
      };
    };
}

{
  description = "scriptor dev shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
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
        ];
      };
    };
}

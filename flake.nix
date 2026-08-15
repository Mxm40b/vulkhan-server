{
  description = "devenv";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        devShells.default = pkgs.mkShell {
          name = "devenv";

          buildInputs = with pkgs; [
            libclang
            pkg-config
            clang
            glibc.dev
            cargo
            rust-analyzer
            clippy
            rustc
            rustfmt
          ];

          shellHook = ''
            export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
            export BINDGEN_EXTRA_CLANG_ARGS="-isystem ${pkgs.glibc.dev}/include"

            exec $(getent passwd "$USER" | cut -d: -f7)
          '';
        };
      }
    );
}

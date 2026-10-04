{
  description = "mbx: a shared build cache for Rust projects";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          manifest = builtins.fromTOML (builtins.readFile ./crates/mbx/Cargo.toml);
          mbx = pkgs.rustPlatform.buildRustPackage {
            pname = "mbx";
            version = manifest.package.version;
            src = pkgs.lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [
              "--package"
              "mbx"
            ];
            nativeBuildInputs = [
              pkgs.cmake
              pkgs.makeWrapper
            ];
            dontUseCmakeConfigure = true;

            postInstall = ''
              makeWrapper "$out/bin/mbx" "$out/libexec/mbx/cargo" \
                --set MBX_CARGO_SHIM_MODE 1 \
                --set MBX_CARGO_SHIM_PATH "$out/libexec/mbx/cargo"
            '';

            # Integration tests build fixture projects and need registry access.
            doCheck = false;
            doInstallCheck = true;
            installCheckPhase = ''
              runHook preInstallCheck
              "$out/bin/mbx" --version
              runHook postInstallCheck
            '';

            meta = {
              description = manifest.package.description;
              homepage = "https://github.com/jdx/mr-boxington";
              license = pkgs.lib.licenses.mit;
              mainProgram = "mbx";
              platforms = systems;
            };
          };
        in
        {
          inherit mbx;
          default = mbx;
        }
      );

      apps = forAllSystems (
        system:
        let
          mbx = {
            type = "app";
            program = nixpkgs.lib.getExe self.packages.${system}.mbx;
            meta.description = "A build cache for Rust projects";
          };
        in
        {
          inherit mbx;
          default = mbx;
        }
      );
    };
}

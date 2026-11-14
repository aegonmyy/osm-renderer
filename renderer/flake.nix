{
  description = "osm_renderer core module — raster map renderer (PBF -> spatial index -> PNG tiles)";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    nixpkgs.follows = "logos-module-builder/nixpkgs";
    # Provides logos-lidl-gen (the contract->scaffold generator the builder
    # runs) and the SDK the crate links. One extra input vs a C++ module.
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk/49dbb0768a9a18cfaf264cc3ecfa55cca2630157";
    logos-rust-sdk.inputs.logos-module-builder.follows = "logos-module-builder";
  };
  outputs = inputs@{ self, logos-module-builder, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
    in {
      packages = forAllSystems (system:
        (logos-module-builder.lib.mkLogosModule {
          src = ./.;
          configFile = ./metadata.json;
          flakeInputs = inputs;
        }).packages.${system});
    };
}

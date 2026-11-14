{
  description = "osm_renderer — 'Map Viewer' Basecamp app. Fetches an OSM region PBF from Logos Storage by CID (via the osm module) and renders it as a pannable/zoomable slippy map (via the osm_renderer_core module).";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    nixpkgs.follows = "logos-module-builder/nixpkgs";
    # The SDK module this app depends on (metadata.json dependencies: ["osm"]).
    logos-osm.url = "github:aegonmyy/logos-osm";
    logos-osm.inputs.logos-module-builder.follows = "logos-module-builder";
    # The renderer core module this app depends on
    # (metadata.json dependencies: ["osm_renderer_core"]). It lives in this
    # same repo at ../renderer; ?dir selects that flake from the repo.
    osm-renderer-core.url = "github:aegonmyy/osm-renderer?dir=renderer";
    osm-renderer-core.inputs.logos-module-builder.follows = "logos-module-builder";
    osm-renderer-core.inputs.logos-rust-sdk.follows = "logos-rust-sdk";
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk";
  };

  outputs = inputs@{ logos-module-builder, logos-osm, osm-renderer-core, ... }:
    logos-module-builder.lib.mkLogosQmlModule {
      src = ./.;
      configFile = ./metadata.json;
      # `osm` and `osm_renderer_core` are the names metadata.json declares as
      # dependencies; the builder resolves their interfaces from these inputs.
      # osm_renderer_core is built from the renderer/ flake in this same repo.
      flakeInputs = inputs // {
        osm = logos-osm;
        osm_renderer_core = osm-renderer-core;
      };
    };
}

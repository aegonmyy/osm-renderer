{
  description = "osm_renderer — a Basecamp 'Map Viewer' app that renders an OSM region fetched from Logos Storage by CID as a pannable/zoomable slippy map.";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    nixpkgs.follows = "logos-module-builder/nixpkgs";
    # The SDK module this app depends on (metadata.json "dependencies": ["osm"]).
    # The app forwards osm ops (regions/lookup/fetch/catalog) to this module
    # over RemoteObjects and renders the fetched PBF locally.
    logos-osm.url = "github:aegonmyy/logos-osm";
    logos-osm.inputs.logos-module-builder.follows = "logos-module-builder";
  };

  outputs = inputs@{ logos-module-builder, logos-osm, ... }:
    logos-module-builder.lib.mkLogosQmlModule {
      src = ./.;
      configFile = ./metadata.json;
      # `osm` is the name metadata.json declares as a dependency; the builder
      # resolves the dependency's interface from this input.
      flakeInputs = inputs // { osm = logos-osm; };
    };
}

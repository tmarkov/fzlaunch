{
  description = "fzf-based launcher, command composer and system entry point";

  inputs = {
    utils.url = "github:numtide/flake-utils";
  };
  
  outputs = { self, nixpkgs, utils }: utils.lib.eachDefaultSystem (system:
  let
    pkgs = import nixpkgs { inherit system; };
  in {
    defaultPackage = pkgs.callPackage ./. { };
  });
}

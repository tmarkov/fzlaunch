{ pkgs ? import <nixpkgs> { } }:
let
  inherit (pkgs) lib;
in
pkgs.python3Packages.buildPythonApplication rec {
  pname = "fzlaunch";
  version = "0.1";

  src = ./.;

  propagatedBuildInputs = with pkgs.python3Packages; [
    pyxdg
    recoll
  ];

  buildInputs = with pkgs; [
    bash
    findutils
    fzf
    poppler_utils
    less
    lesspipe
  ];

  checkPhase = "echo 1";
  doInstallCheck = false;
}

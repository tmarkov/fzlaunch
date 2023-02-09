{ lib
, stdenv
, makeWrapper
, fetchFromGitHub

# for fzlaunch executable and entry_manager.py
, bash
, fzf
, python310
, coreutils
, gtk3

# for builtin modules
, findutils
, poppler_utils
, less
, lesspipe
}:
let
  python-packages = python: with python; [
    pyxdg
    recoll
  ];
  fzl_python = python310.withPackages python-packages;
in
stdenv.mkDerivation rec {
  pname = "fzlaunch";
  version = "main";

  buildInputs = [
    bash
    fzf
    fzl_python
    coreutils
    gtk3
    
    findutils
    poppler_utils
    less
    lesspipe
  ];

  nativeBuildInputs = [
    makeWrapper
  ];

  src = ./.;
    
  installPhase = ''
    mkdir -p $out
    cp -r $src $out/bin
    chmod +w $out/bin
    wrapProgram $out/bin/fzlaunch \
      --set FZLAUNCH_EXTRA_PATH ${lib.makeBinPath buildInputs}
  '';
}

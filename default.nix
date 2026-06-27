{
  lib,
  naersk,
}:

naersk.buildPackage {
  pname = "fzlaunch";
  version = "0.1.0";

  src = lib.cleanSource ./.;
}

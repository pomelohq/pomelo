{ pkgs ? import <nixpkgs> { } }:

let
  name = "web";
  version = "1.0.0";
in
pkgs.stdenv.mkDerivation {
  pname = name;
  inherit version;
  src = ./.;
  buildInputs = with pkgs; [ nodejs git ];
  meta.description = "The ${name} app";
}

{
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
  zstd,
  libGL,
  libxkbcommon,
  wayland,
  xorg,
}:

let
  # eframe dlopens its windowing and GL libraries on Linux, so they must be on the runtime path.
  linuxRuntimeLibraries = [
    libGL
    libxkbcommon
    wayland
    xorg.libX11
    xorg.libXcursor
    xorg.libXi
    xorg.libXrandr
  ];
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "spectra";
  version = (lib.importTOML ./crates/app/Cargo.toml).package.version;

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./crates
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ pkg-config ];

  buildInputs = [ zstd ] ++ lib.optionals stdenv.hostPlatform.isLinux linuxRuntimeLibraries;

  env.ZSTD_SYS_USE_PKG_CONFIG = "1";

  cargoBuildFlags = [
    "--package"
    "spectra"
  ];

  cargoTestFlags = [ "--workspace" ];

  postFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
    patchelf --add-rpath ${lib.makeLibraryPath linuxRuntimeLibraries} $out/bin/spectra
  '';

  meta = {
    description = "Spectrum analyzer and waterfall for SDR devices and SigMF recordings";
    license = lib.licenses.mit;
    mainProgram = "spectra";
    platforms = lib.platforms.darwin ++ lib.platforms.linux;
  };
})

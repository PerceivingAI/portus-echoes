# Build & Packaging Guide

This guide covers building PortusEchoes from source and generating release installers across supported platforms.

---

## 1. System Build Prerequisites

### 1.1. Common Requirements
- **Node.js:** v18.0.0 or higher with `npm`.
- **Rust Toolchain:** 1.88.0+ (`rustup default stable`).
- **CMake:** 3.16 or higher (required for native `whisper.cpp` build).

### 1.2. Linux Dependencies

Install native C library headers, audio subsystem libraries, and Vulkan loaders:

```bash
# Arch Linux
sudo pacman -S base-devel cmake alsa-lib pipewire vulkan-loader vulkan-headers libx11 libxtst libxkbcommon

# Ubuntu / Debian
sudo apt-get install build-essential cmake libasound2-dev libvulkan-dev libx11-dev libxtst-dev libxkbcommon-dev

# Fedora
sudo dnf install @development-tools cmake alsa-lib-devel vulkan-loader-devel libX11-devel libXtst-devel libxkbcommon-devel
```

### 1.3. Windows Dependencies
- **Visual Studio 2022:** With the "Desktop development with C++" workload installed.
- **Vulkan SDK (Optional):** Required only if compiling custom Vulkan shader binaries. The bundled build compiles against system Vulkan headers automatically.

---

## 2. Compilation Workflow

1. **Bootstrap Native Engine:**
   When running `npm install`, the post-install hook automatically runs `scripts/bootstrap-whisper.mjs`. This downloads and builds pinned `whisper.cpp v1.9.1` into the `.deps/` directory:
   ```bash
   npm install
   ```

2. **Environment Configuration:**
   Copy the example environment file:
   ```bash
   cp .env.example .env
   ```

3. **Development Launch:**
   Starts Vite and the Tauri desktop development binary:
   ```bash
   npm run tauri dev
   ```

4. **Production Build:**
   Compiles optimized production bundles:
   ```bash
   npm run tauri build
   ```

---

## 3. Packaging Targets

Upon executing `npm run tauri build`, installers are output to `src-tauri/target/release/bundle/`:

### 3.1. Linux Targets
- **AppImage:** Single self-contained executable for distribution across Linux distributions.
- **Debian Package (`.deb`):** Standard installation package for Ubuntu/Debian systems.

### 3.2. Windows Targets
- **MSI Installer (`.msi`):** Enterprise-ready Windows installer.
- **NSIS Setup (`.exe`):** Lightweight standalone Windows installer.

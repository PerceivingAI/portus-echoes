import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as tar from "tar";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_REPO_ROOT = path.resolve(SCRIPT_DIR, "..");
const MANIFEST_RELATIVE_PATH = "dependencies/whisper.json";
const MARKER_FILE = ".portus-dependency.json";

function normalizeRelative(value) {
  return value.replaceAll("\\", "/").replace(/^\.\//, "").replace(/\/$/, "");
}

function sha256Buffer(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

async function sha256File(filePath) {
  const hash = crypto.createHash("sha256");
  await new Promise((resolve, reject) => {
    const stream = fs.createReadStream(filePath);
    stream.on("data", chunk => hash.update(chunk));
    stream.on("error", reject);
    stream.on("end", resolve);
  });
  return hash.digest("hex");
}

async function loadManifest(repoRoot = DEFAULT_REPO_ROOT) {
  const manifestPath = path.join(repoRoot, MANIFEST_RELATIVE_PATH);
  const raw = await fsp.readFile(manifestPath);
  const manifest = JSON.parse(raw.toString("utf8"));

  assert.equal(manifest.schemaVersion, 1, "unsupported whisper dependency manifest schema");
  assert.equal(manifest.name, "whisper.cpp", "unexpected dependency manifest name");
  assert.match(manifest.commit, /^[a-f0-9]{40}$/, "whisper commit must be an immutable 40-character SHA");
  assert.match(manifest.archiveSha256, /^[a-f0-9]{64}$/, "whisper archive SHA-256 is invalid");
  assert.ok(Number.isInteger(manifest.archiveBytes) && manifest.archiveBytes > 0, "archive byte count is invalid");
  assert.equal(manifest.preparedDir, ".deps/whisper.cpp", "prepared whisper path is not authoritative");
  assert.equal(manifest.transform, "whisper-cmake-cpu-vulkan-win-native-v3", "unknown whisper source transform");
  assert.ok(Array.isArray(manifest.files) && manifest.files.length > 0, "source file allowlist is empty");
  assert.ok(Array.isArray(manifest.trees) && manifest.trees.length > 0, "source tree allowlist is empty");
  assert.ok(Array.isArray(manifest.requiredFiles) && manifest.requiredFiles.length > 0, "required source list is empty");
  assert.ok(Array.isArray(manifest.prohibitedPaths), "prohibited source list is invalid");

  return {
    manifest,
    manifestSha256: sha256Buffer(raw),
    manifestPath,
  };
}

function archiveRelativePath(entryPath, manifest) {
  const normalized = normalizeRelative(entryPath);
  const root = `${normalizeRelative(manifest.archiveRoot)}/`;
  if (!normalized.startsWith(root)) return null;
  return normalizeRelative(normalized.slice(root.length));
}

function isAllowedSource(relativePath, manifest) {
  const normalized = normalizeRelative(relativePath);
  if (!normalized) return true;
  if (manifest.files.some(item => normalizeRelative(item) === normalized)) return true;
  return manifest.trees.some(tree => {
    const prefix = normalizeRelative(tree);
    return normalized === prefix || normalized.startsWith(`${prefix}/`);
  });
}

function isAllowedDirectory(relativePath, manifest) {
  const normalized = normalizeRelative(relativePath);
  if (!normalized) return true;
  if (isAllowedSource(normalized, manifest)) return true;
  const candidates = [...manifest.files, ...manifest.trees].map(normalizeRelative);
  return candidates.some(candidate => candidate.startsWith(`${normalized}/`));
}

function replaceExactOnce(text, search, replacement, label) {
  const first = text.indexOf(search);
  if (first < 0 || text.indexOf(search, first + search.length) >= 0) {
    throw new Error(`upstream source transform mismatch: ${label}`);
  }
  return `${text.slice(0, first)}${replacement}${text.slice(first + search.length)}`;
}

function removeBetweenOnce(text, start, end, label) {
  const startIndex = text.indexOf(start);
  if (startIndex < 0 || text.indexOf(start, startIndex + start.length) >= 0) {
    throw new Error(`upstream source transform mismatch: ${label} start`);
  }
  const endIndex = text.indexOf(end, startIndex + start.length);
  if (endIndex < 0) {
    throw new Error(`upstream source transform mismatch: ${label} end`);
  }
  const after = endIndex + end.length;
  return `${text.slice(0, startIndex)}${text.slice(after)}`;
}

export async function applySourceTransform(preparedRoot, transformName) {
  if (transformName !== "whisper-cmake-cpu-vulkan-win-native-v3") {
    throw new Error(`unsupported whisper transform: ${transformName}`);
  }

  const rootCmakePath = path.join(preparedRoot, "CMakeLists.txt");
  let rootCmake = await fsp.readFile(rootCmakePath, "utf8");
  rootCmake = replaceExactOnce(
    rootCmake,
    `if (CMAKE_SOURCE_DIR STREQUAL CMAKE_CURRENT_SOURCE_DIR)\n    set(WHISPER_STANDALONE ON)\n\n    include(git-vars)\n\n    # configure project version\n    configure_file(\${CMAKE_SOURCE_DIR}/bindings/javascript/package-tmpl.json \${CMAKE_SOURCE_DIR}/bindings/javascript/package.json @ONLY)\nelse()\n    set(WHISPER_STANDALONE OFF)\nendif()`,
    `set(WHISPER_STANDALONE OFF)`,
    "disable standalone upstream repository behavior",
  );
  rootCmake = replaceExactOnce(
    rootCmake,
    `set_target_properties(parakeet PROPERTIES PUBLIC_HEADER \${CMAKE_CURRENT_SOURCE_DIR}/include/parakeet.h)\ninstall(TARGETS parakeet LIBRARY PUBLIC_HEADER)\n\ntarget_compile_definitions(parakeet PRIVATE\n    PARAKEET_VERSION="\${PROJECT_VERSION}"\n)\n`,
    "",
    "remove Parakeet install target",
  );
  rootCmake = removeBetweenOnce(
    rootCmake,
    `set(PARAKEET_INCLUDE_INSTALL_DIR`,
    `install(FILES "\${CMAKE_CURRENT_BINARY_DIR}/parakeet.pc"\n        DESTINATION \${CMAKE_INSTALL_LIBDIR}/pkgconfig)`,
    "remove Parakeet package metadata",
  );
  await fsp.writeFile(rootCmakePath, rootCmake);

  const srcCmakePath = path.join(preparedRoot, "src", "CMakeLists.txt");
  let srcCmake = await fsp.readFile(srcCmakePath, "utf8");
  srcCmake = removeBetweenOnce(
    srcCmake,
    `add_library(parakeet`,
    `target_link_libraries(parakeet PUBLIC ggml Threads::Threads)`,
    "remove Parakeet library",
  );
  srcCmake = replaceExactOnce(
    srcCmake,
    `set_target_properties(parakeet PROPERTIES\n    VERSION \${PROJECT_VERSION}\n    SOVERSION \${SOVERSION}\n)\n`,
    "",
    "remove Parakeet target properties",
  );
  srcCmake = replaceExactOnce(
    srcCmake,
    `    set(PARAKEET_EXTRA_FLAGS \${PARAKEET_EXTRA_FLAGS} -DPARAKEET_BIG_ENDIAN)\n`,
    "",
    "remove Parakeet big-endian flags",
  );
  srcCmake = replaceExactOnce(
    srcCmake,
    `if (PARAKEET_EXTRA_FLAGS)\n    target_compile_options(parakeet PRIVATE \${PARAKEET_EXTRA_FLAGS})\nendif()\n`,
    "",
    "remove Parakeet compile flags",
  );
  srcCmake = replaceExactOnce(
    srcCmake,
    `    set_target_properties(parakeet PROPERTIES POSITION_INDEPENDENT_CODE ON)\n    target_compile_definitions(parakeet PRIVATE PARAKEET_SHARED PARAKEET_BUILD)\n`,
    "",
    "remove Parakeet shared-library setup",
  );
  const vulkanCmakePath = path.join(preparedRoot, "ggml", "src", "ggml-vulkan", "CMakeLists.txt");
  let vulkanCmake = await fsp.readFile(vulkanCmakePath, "utf8");
  vulkanCmake = replaceExactOnce(
    vulkanCmake,
    `    include(ExternalProject)\n\n    if (CMAKE_CROSSCOMPILING)\n        list(APPEND VULKAN_SHADER_GEN_CMAKE_ARGS -DCMAKE_TOOLCHAIN_FILE=\${HOST_CMAKE_TOOLCHAIN_FILE})\n        message(STATUS "vulkan-shaders-gen toolchain file: \${HOST_CMAKE_TOOLCHAIN_FILE}")\n    endif()\n\n    ExternalProject_Add(\n        vulkan-shaders-gen\n        SOURCE_DIR \${CMAKE_CURRENT_SOURCE_DIR}/vulkan-shaders\n        CMAKE_ARGS -DCMAKE_INSTALL_PREFIX=\${CMAKE_BINARY_DIR}/$<CONFIG>\n                   -DCMAKE_INSTALL_BINDIR=.\n                   -DCMAKE_BUILD_TYPE=$<CONFIG>\n                   \${VULKAN_SHADER_GEN_CMAKE_ARGS}\n\n        BUILD_COMMAND \${CMAKE_COMMAND} --build . --config $<CONFIG>\n        BUILD_ALWAYS  TRUE\n\n        # NOTE: When DESTDIR is set using Makefile generators and\n        # "make install" triggers the build step, vulkan-shaders-gen\n        # would be installed into the DESTDIR prefix, so it is unset\n        # to ensure that does not happen.\n\n        INSTALL_COMMAND \${CMAKE_COMMAND} -E env --unset=DESTDIR\n                        \${CMAKE_COMMAND} --install . --config $<CONFIG>\n    )\n\n    set (_ggml_vk_host_suffix $<IF:$<STREQUAL:\${CMAKE_HOST_SYSTEM_NAME},Windows>,.exe,>)\n    set (_ggml_vk_genshaders_dir "\${CMAKE_BINARY_DIR}/$<CONFIG>")\n    set (_ggml_vk_genshaders_cmd "\${_ggml_vk_genshaders_dir}/vulkan-shaders-gen\${_ggml_vk_host_suffix}")`,
    `    if (WIN32 AND NOT CMAKE_CROSSCOMPILING)\n        add_executable(vulkan-shaders-gen vulkan-shaders/vulkan-shaders-gen.cpp)\n        target_compile_features(vulkan-shaders-gen PRIVATE cxx_std_17)\n        target_link_libraries(vulkan-shaders-gen PUBLIC Threads::Threads)\n        foreach(_portus_vk_feature\n                GGML_VULKAN_COOPMAT_GLSLC_SUPPORT\n                GGML_VULKAN_COOPMAT2_GLSLC_SUPPORT\n                GGML_VULKAN_COOPMAT2_DECODE_VECTOR_GLSLC_SUPPORT\n                GGML_VULKAN_INTEGER_DOT_GLSLC_SUPPORT\n                GGML_VULKAN_BFLOAT16_GLSLC_SUPPORT\n                GGML_VULKAN_SHADER_DEBUG_INFO)\n            if (\${_portus_vk_feature})\n                target_compile_definitions(vulkan-shaders-gen PRIVATE \${_portus_vk_feature})\n            endif()\n        endforeach()\n        set (_ggml_vk_genshaders_cmd "$<TARGET_FILE:vulkan-shaders-gen>")\n    else()\n        include(ExternalProject)\n\n        if (CMAKE_CROSSCOMPILING)\n            list(APPEND VULKAN_SHADER_GEN_CMAKE_ARGS -DCMAKE_TOOLCHAIN_FILE=\${HOST_CMAKE_TOOLCHAIN_FILE})\n            message(STATUS "vulkan-shaders-gen toolchain file: \${HOST_CMAKE_TOOLCHAIN_FILE}")\n        endif()\n\n        ExternalProject_Add(\n            vulkan-shaders-gen\n            SOURCE_DIR \${CMAKE_CURRENT_SOURCE_DIR}/vulkan-shaders\n            CMAKE_ARGS -DCMAKE_INSTALL_PREFIX=\${CMAKE_BINARY_DIR}/$<CONFIG>\n                       -DCMAKE_INSTALL_BINDIR=.\n                       -DCMAKE_BUILD_TYPE=$<CONFIG>\n                       \${VULKAN_SHADER_GEN_CMAKE_ARGS}\n\n            BUILD_COMMAND \${CMAKE_COMMAND} --build . --config $<CONFIG>\n            BUILD_ALWAYS  TRUE\n\n            INSTALL_COMMAND \${CMAKE_COMMAND} -E env --unset=DESTDIR\n                            \${CMAKE_COMMAND} --install . --config $<CONFIG>\n        )\n\n        set (_ggml_vk_host_suffix $<IF:$<STREQUAL:\${CMAKE_HOST_SYSTEM_NAME},Windows>,.exe,>)\n        set (_ggml_vk_genshaders_dir "\${CMAKE_BINARY_DIR}/$<CONFIG>")\n        set (_ggml_vk_genshaders_cmd "\${_ggml_vk_genshaders_dir}/vulkan-shaders-gen\${_ggml_vk_host_suffix}")\n    endif()`,
    "build Vulkan shader generator directly in native Windows parent build",
  );
  await fsp.writeFile(vulkanCmakePath, vulkanCmake);

  await fsp.writeFile(srcCmakePath, srcCmake);
}

async function walkPrepared(root, relative = "") {
  const directory = path.join(root, relative);
  const entries = await fsp.readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const childRelative = normalizeRelative(path.posix.join(relative.replaceAll("\\", "/"), entry.name));
    const childPath = path.join(root, childRelative);
    const stat = await fsp.lstat(childPath);
    if (stat.isSymbolicLink()) throw new Error(`prepared dependency contains a symbolic link: ${childRelative}`);
    if (entry.isDirectory()) files.push(...await walkPrepared(root, childRelative));
    else if (entry.isFile()) files.push(childRelative);
    else throw new Error(`prepared dependency contains unsupported filesystem entry: ${childRelative}`);
  }
  return files;
}

export async function validatePreparedTree(preparedRoot, manifest, { markerRequired = true } = {}) {
  for (const required of manifest.requiredFiles) {
    const requiredPath = path.join(preparedRoot, normalizeRelative(required));
    let stat;
    try {
      stat = await fsp.stat(requiredPath);
    } catch {
      throw new Error(`prepared whisper dependency is missing required source: ${required}`);
    }
    if (!stat.isFile()) throw new Error(`required whisper source is not a file: ${required}`);
  }

  for (const prohibited of manifest.prohibitedPaths) {
    const prohibitedPath = path.join(preparedRoot, normalizeRelative(prohibited));
    if (fs.existsSync(prohibitedPath)) {
      throw new Error(`prepared whisper dependency contains prohibited source: ${prohibited}`);
    }
  }

  const files = await walkPrepared(preparedRoot);
  for (const relative of files) {
    if (relative === MARKER_FILE) continue;
    if (!isAllowedSource(relative, manifest)) {
      throw new Error(`prepared whisper dependency contains non-allowlisted source: ${relative}`);
    }
  }

  const cmakeFiles = files.filter(relative => path.basename(relative).toLowerCase() === "cmakelists.txt" || relative.toLowerCase().endsWith(".cmake"));
  for (const relative of cmakeFiles) {
    const content = await fsp.readFile(path.join(preparedRoot, relative), "utf8");
    if (/parakeet/i.test(content)) {
      throw new Error(`prepared whisper build graph still references Parakeet: ${relative}`);
    }
  }

  if (markerRequired && !fs.existsSync(path.join(preparedRoot, MARKER_FILE))) {
    throw new Error("prepared whisper dependency has no authoritative marker");
  }
  return files;
}

async function downloadArchive(url, outputPath) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok || !response.body) {
    throw new Error(`failed to download whisper.cpp source archive: HTTP ${response.status}`);
  }
  const file = fs.createWriteStream(outputPath, { flags: "wx" });
  try {
    for await (const chunk of response.body) {
      if (!file.write(chunk)) await new Promise(resolve => file.once("drain", resolve));
    }
    await new Promise((resolve, reject) => file.end(error => error ? reject(error) : resolve()));
  } catch (error) {
    file.destroy();
    throw error;
  }
}

async function extractAllowlistedArchive(archivePath, destination, manifest) {
  await fsp.mkdir(destination, { recursive: true });
  await tar.x({
    file: archivePath,
    cwd: destination,
    strip: 1,
    preservePaths: false,
    filter(entryPath, entry) {
      const relative = archiveRelativePath(entryPath, manifest);
      if (relative === null) return false;
      if (entry?.type === "SymbolicLink" || entry?.type === "Link") return false;
      if (entry?.type === "Directory") return isAllowedDirectory(relative, manifest);
      return isAllowedSource(relative, manifest);
    },
  });
}

async function readMarker(preparedRoot) {
  try {
    return JSON.parse(await fsp.readFile(path.join(preparedRoot, MARKER_FILE), "utf8"));
  } catch {
    return null;
  }
}

function markerMatches(marker, manifest, manifestSha256) {
  return marker?.schemaVersion === 1
    && marker?.name === manifest.name
    && marker?.release === manifest.release
    && marker?.commit === manifest.commit
    && marker?.archiveSha256 === manifest.archiveSha256
    && marker?.transformVersion === manifest.transformVersion
    && marker?.manifestSha256 === manifestSha256;
}

async function replacePreparedTreeAtomically(stagePrepared, preparedRoot) {
  const parent = path.dirname(preparedRoot);
  const backup = path.join(parent, `.whisper-backup-${process.pid}-${crypto.randomUUID()}`);
  let hadExisting = false;
  try {
    if (fs.existsSync(preparedRoot)) {
      await fsp.rename(preparedRoot, backup);
      hadExisting = true;
    }
    await fsp.rename(stagePrepared, preparedRoot);
    if (hadExisting) await fsp.rm(backup, { recursive: true, force: true });
  } catch (error) {
    if (!fs.existsSync(preparedRoot) && hadExisting && fs.existsSync(backup)) {
      await fsp.rename(backup, preparedRoot);
    }
    throw error;
  } finally {
    if (fs.existsSync(backup)) await fsp.rm(backup, { recursive: true, force: true });
  }
}

export async function bootstrapDependency({
  repoRoot = DEFAULT_REPO_ROOT,
  archivePath = process.env.PORTUS_WHISPER_ARCHIVE || null,
  quiet = false,
} = {}) {
  const { manifest, manifestSha256 } = await loadManifest(repoRoot);
  const preparedRoot = path.join(repoRoot, manifest.preparedDir);
  const marker = await readMarker(preparedRoot);
  if (markerMatches(marker, manifest, manifestSha256)) {
    try {
      await validatePreparedTree(preparedRoot, manifest);
      if (!quiet) console.log(`whisper.cpp ${manifest.release} already prepared`);
      return { status: "ready", preparedRoot, manifest };
    } catch {
      // A stale/corrupt prepared tree is rebuilt below; it is never trusted.
    }
  }

  const depsRoot = path.join(repoRoot, ".deps");
  await fsp.mkdir(depsRoot, { recursive: true });
  const nonce = `${process.pid}-${crypto.randomUUID()}`;
  const stageRoot = path.join(depsRoot, `.whisper-stage-${nonce}`);
  const stagePrepared = path.join(stageRoot, "whisper.cpp");
  const downloadedArchive = path.join(depsRoot, `.whisper-${nonce}.tar.gz`);
  let sourceArchive = archivePath ? path.resolve(repoRoot, archivePath) : downloadedArchive;

  try {
    if (!archivePath) {
      if (!quiet) console.log(`Downloading pinned whisper.cpp ${manifest.release} source...`);
      await downloadArchive(manifest.archiveUrl, downloadedArchive);
    } else if (!fs.existsSync(sourceArchive)) {
      throw new Error(`local whisper archive does not exist: ${sourceArchive}`);
    }

    const stat = await fsp.stat(sourceArchive);
    if (stat.size !== manifest.archiveBytes) {
      throw new Error(`whisper.cpp archive size mismatch: expected ${manifest.archiveBytes}, got ${stat.size}`);
    }
    const actualSha256 = await sha256File(sourceArchive);
    if (actualSha256 !== manifest.archiveSha256) {
      throw new Error(`whisper.cpp archive SHA-256 mismatch: expected ${manifest.archiveSha256}, got ${actualSha256}`);
    }

    await extractAllowlistedArchive(sourceArchive, stagePrepared, manifest);
    await applySourceTransform(stagePrepared, manifest.transform);
    await validatePreparedTree(stagePrepared, manifest, { markerRequired: false });

    const markerContent = {
      schemaVersion: 1,
      name: manifest.name,
      release: manifest.release,
      commit: manifest.commit,
      archiveSha256: manifest.archiveSha256,
      transformVersion: manifest.transformVersion,
      manifestSha256,
    };
    await fsp.writeFile(path.join(stagePrepared, MARKER_FILE), `${JSON.stringify(markerContent, null, 2)}\n`);
    await validatePreparedTree(stagePrepared, manifest);
    await replacePreparedTreeAtomically(stagePrepared, preparedRoot);

    if (!quiet) console.log(`Prepared whisper.cpp ${manifest.release} at ${path.relative(repoRoot, preparedRoot)}`);
    return { status: "prepared", preparedRoot, manifest };
  } finally {
    await fsp.rm(stageRoot, { recursive: true, force: true });
    if (!archivePath) await fsp.rm(downloadedArchive, { force: true });
  }
}

function parseCliArguments(argv) {
  const result = {};
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === "--archive") {
      const next = argv[index + 1];
      if (!next) throw new Error("--archive requires a path");
      result.archivePath = next;
      index += 1;
    } else if (value === "--quiet") {
      result.quiet = true;
    } else {
      throw new Error(`unknown bootstrap argument: ${value}`);
    }
  }
  return result;
}

const invokedPath = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : null;
if (invokedPath === import.meta.url) {
  bootstrapDependency(parseCliArguments(process.argv.slice(2))).catch(error => {
    console.error(`Dependency bootstrap failed: ${error.message}`);
    process.exitCode = 1;
  });
}

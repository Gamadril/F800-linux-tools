
const TARGET_W = 1024;
const TARGET_H = 600;
const PART_SIZE = 131072; // 128 KiB

// Offscreen canvas holds the clean 1024x600 output without UI grid lines
const renderCanvas = document.createElement("canvas");
renderCanvas.width = TARGET_W;
renderCanvas.height = TARGET_H;
const renderCtx = renderCanvas.getContext("2d", { alpha: false });

const editorCanvas = document.getElementById("editorCanvas");
const editorCtx = editorCanvas.getContext("2d", { alpha: false });

const state = {
  wasm: null,
  img: null,
  sourceName: "test_pattern",
  mode: "cover", // "cover" | "contain" | "stretch"
  zoom: 1.0,
  panX: 0,
  panY: 0,
  rotation: 0, // degrees: 0, 90, 180, 270
  flipH: false,
  flipV: false,
  bgColor: "#000000",
  quality: 0.92,
  showGrid: true,
  dragging: false,
  dragStartX: 0,
  dragStartY: 0,
  lastJpegBytes: null,
};

// UI Elements
const wasmStatusBadge = document.getElementById("wasmStatusBadge");
const dropzone = document.getElementById("dropzone");
const fileInput = document.getElementById("fileInput");
const openBtn = document.getElementById("openBtn");
const sampleBtn = document.getElementById("sampleBtn");
const fileLabel = document.getElementById("fileLabel");
const sourceDimPill = document.getElementById("sourceDimPill");
const gridBtn = document.getElementById("gridBtn");
const downloadJpgBtn = document.getElementById("downloadJpgBtn");
const modeLabel = document.getElementById("modeLabel");
const zoomRange = document.getElementById("zoomRange");
const zoomValue = document.getElementById("zoomValue");
const rotLeftBtn = document.getElementById("rotLeftBtn");
const rotRightBtn = document.getElementById("rotRightBtn");
const flipHBtn = document.getElementById("flipHBtn");
const flipVBtn = document.getElementById("flipVBtn");
const centerBtn = document.getElementById("centerBtn");
const bgColorInput = document.getElementById("bgColorInput");
const bgColorHex = document.getElementById("bgColorHex");
const outFileName = document.getElementById("outFileName");
const qualityRange = document.getElementById("qualityRange");
const qualityValue = document.getElementById("qualityValue");
const jpegSizeText = document.getElementById("jpegSizeText");
const jpegMeterFill = document.getElementById("jpegMeterFill");
const jpegPctText = document.getElementById("jpegPctText");
const generateImgBtn = document.getElementById("generateImgBtn");
const statusBox = document.getElementById("statusBox");

function base64ToUint8Array(b64) {
  const bin = atob(b64);
  const arr = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) {
    arr[i] = bin.charCodeAt(i);
  }
  return arr;
}

function dataUrlToUint8Array(dataUrl) {
  const idx = dataUrl.indexOf(",");
  return base64ToUint8Array(dataUrl.slice(idx + 1));
}

async function initWasm() {
  try {
    const resp = await fetch("./img_tool.wasm");
    if (!resp.ok) throw new Error(`HTTP ${resp.status} loading img_tool.wasm`);
    const wasmBytes = new Uint8Array(await resp.arrayBuffer());
    const { instance } = await WebAssembly.instantiate(wasmBytes, {});
    state.wasm = instance.exports;
    const w = state.wasm.wasm_default_width();
    const h = state.wasm.wasm_default_height();
    wasmStatusBadge.textContent = `WASM Ready (${w}×${h})`;
    wasmStatusBadge.className = "badge ok";
    generateImgBtn.disabled = false;
  } catch (err) {
    wasmStatusBadge.textContent = "WASM Load Error";
    wasmStatusBadge.className = "badge err";
    setStatus(`Failed to initialize img_tool.wasm: ${err.message}`, "err");
  }
}

function setStatus(html, type = "") {
  statusBox.className = `status-box ${type}`;
  statusBox.innerHTML = html;
}

function callWasmBuildImg(jpegBytes) {
  const wasm = state.wasm;
  const ptr = wasm.wasm_alloc(jpegBytes.length);
  try {
    new Uint8Array(wasm.memory.buffer, ptr, jpegBytes.length).set(jpegBytes);
    const rc = wasm.wasm_build_bootlogo_img(ptr, jpegBytes.length, TARGET_W, TARGET_H, PART_SIZE, 1);
    if (rc !== 0) {
      const errPtr = wasm.wasm_error_ptr();
      const errLen = wasm.wasm_error_len();
      const msg = new TextDecoder().decode(new Uint8Array(wasm.memory.buffer, errPtr, errLen));
      throw new Error(msg || "wasm_build_bootlogo_img failed");
    }
    const outPtr = wasm.wasm_output_ptr();
    const outLen = wasm.wasm_output_len();
    return new Uint8Array(wasm.memory.buffer.slice(outPtr, outPtr + outLen));
  } finally {
    wasm.wasm_dealloc(ptr, jpegBytes.length);
  }
}

function callWasmExtractJpg(rawBytes) {
  const wasm = state.wasm;
  const ptr = wasm.wasm_alloc(rawBytes.length);
  try {
    new Uint8Array(wasm.memory.buffer, ptr, rawBytes.length).set(rawBytes);
    const rc = wasm.wasm_extract_bootlogo_jpg(ptr, rawBytes.length);
    if (rc !== 0) {
      const errPtr = wasm.wasm_error_ptr();
      const errLen = wasm.wasm_error_len();
      const msg = new TextDecoder().decode(new Uint8Array(wasm.memory.buffer, errPtr, errLen));
      throw new Error(msg || "wasm_extract_bootlogo_jpg failed");
    }
    const outPtr = wasm.wasm_output_ptr();
    const outLen = wasm.wasm_output_len();
    return new Uint8Array(wasm.memory.buffer.slice(outPtr, outPtr + outLen));
  } finally {
    wasm.wasm_dealloc(ptr, rawBytes.length);
  }
}

function createDemoImage() {
  const c = document.createElement("canvas");
  c.width = TARGET_W;
  c.height = TARGET_H;
  const ctx = c.getContext("2d");

  const grad = ctx.createLinearGradient(0, 0, TARGET_W, TARGET_H);
  grad.addColorStop(0, "#0f172a");
  grad.addColorStop(0.5, "#1e3a8a");
  grad.addColorStop(1, "#0284c7");
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, TARGET_W, TARGET_H);

  // Subtle grid circles
  ctx.strokeStyle = "rgba(255,255,255,0.12)";
  ctx.lineWidth = 2;
  for (const r of [120, 220, 320]) {
    ctx.beginPath();
    ctx.arc(TARGET_W / 2, TARGET_H / 2, r, 0, Math.PI * 2);
    ctx.stroke();
  }

  // Corner markers
  ctx.strokeStyle = "#38bdf8";
  ctx.lineWidth = 4;
  ctx.strokeRect(24, 24, TARGET_W - 48, TARGET_H - 48);

  // Title
  ctx.fillStyle = "#ffffff";
  ctx.font = "bold 54px sans-serif";
  ctx.textAlign = "center";
  ctx.fillText("F800 DEVICE", TARGET_W / 2, TARGET_H / 2 - 20);

  ctx.fillStyle = "#bae6fd";
  ctx.font = "28px monospace";
  ctx.fillText("1024 × 600 CUSTOM BOOTLOGO", TARGET_W / 2, TARGET_H / 2 + 34);

  const img = new Image();
  img.onload = () => {
    state.img = img;
    state.sourceName = "bootlogo_test_pattern";
    sourceDimPill.textContent = `Source: ${img.width}×${img.height} (Test Pattern)`;
    resetTransform();
    renderAndMeasure();
  };
  img.src = c.toDataURL("image/png");
}

function resetTransform() {
  state.zoom = 1.0;
  state.panX = 0;
  state.panY = 0;
  state.rotation = 0;
  state.flipH = false;
  state.flipV = false;
  zoomRange.value = 100;
  zoomValue.textContent = "100%";
}

function drawToRenderCanvas() {
  renderCtx.save();
  renderCtx.fillStyle = state.bgColor;
  renderCtx.fillRect(0, 0, TARGET_W, TARGET_H);

  if (state.img) {
    const isRot90 = (Math.abs(state.rotation) % 180) === 90;
    const srcW = isRot90 ? state.img.height : state.img.width;
    const srcH = isRot90 ? state.img.width : state.img.height;

    renderCtx.translate(TARGET_W / 2 + state.panX, TARGET_H / 2 + state.panY);

    if (state.mode === "stretch") {
      renderCtx.scale(
        state.zoom * (state.flipH ? -1 : 1),
        state.zoom * (state.flipV ? -1 : 1)
      );
      renderCtx.rotate((state.rotation * Math.PI) / 180);
      const drawW = isRot90 ? TARGET_H : TARGET_W;
      const drawH = isRot90 ? TARGET_W : TARGET_H;
      renderCtx.drawImage(state.img, -drawW / 2, -drawH / 2, drawW, drawH);
    } else {
      const baseScale =
        state.mode === "cover"
          ? Math.max(TARGET_W / srcW, TARGET_H / srcH)
          : Math.min(TARGET_W / srcW, TARGET_H / srcH);
      const finalScale = baseScale * state.zoom;
      renderCtx.scale(
        finalScale * (state.flipH ? -1 : 1),
        finalScale * (state.flipV ? -1 : 1)
      );
      renderCtx.rotate((state.rotation * Math.PI) / 180);
      renderCtx.drawImage(
        state.img,
        -state.img.width / 2,
        -state.img.height / 2,
        state.img.width,
        state.img.height
      );
    }
  }
  renderCtx.restore();
}

function drawEditorCanvas() {
  editorCtx.drawImage(renderCanvas, 0, 0);

  if (state.showGrid) {
    editorCtx.save();
    editorCtx.strokeStyle = "rgba(255, 255, 255, 0.28)";
    editorCtx.lineWidth = 1;

    // Rule of thirds
    for (let i = 1; i <= 2; i++) {
      const x = Math.round((TARGET_W * i) / 3) + 0.5;
      const y = Math.round((TARGET_H * i) / 3) + 0.5;
      editorCtx.beginPath();
      editorCtx.moveTo(x, 0);
      editorCtx.lineTo(x, TARGET_H);
      editorCtx.stroke();

      editorCtx.beginPath();
      editorCtx.moveTo(0, y);
      editorCtx.lineTo(TARGET_W, y);
      editorCtx.stroke();
    }

    // Center crosshair
    editorCtx.strokeStyle = "rgba(56, 189, 248, 0.5)";
    editorCtx.beginPath();
    editorCtx.moveTo(TARGET_W / 2 - 14, TARGET_H / 2);
    editorCtx.lineTo(TARGET_W / 2 + 14, TARGET_H / 2);
    editorCtx.moveTo(TARGET_W / 2, TARGET_H / 2 - 14);
    editorCtx.lineTo(TARGET_W / 2, TARGET_H / 2 + 14);
    editorCtx.stroke();
    editorCtx.restore();
  }
}

function encodeBaselineJpeg(quality) {
  const dataUrl = renderCanvas.toDataURL("image/jpeg", quality);
  return dataUrlToUint8Array(dataUrl);
}

function renderAndMeasure() {
  drawToRenderCanvas();
  drawEditorCanvas();

  const jpgBytes = encodeBaselineJpeg(state.quality);
  state.lastJpegBytes = jpgBytes;

  const len = jpgBytes.length;
  const pct = Math.min(100, Math.round((len / PART_SIZE) * 100));
  jpegSizeText.textContent = `${len.toLocaleString()} / ${PART_SIZE.toLocaleString()} B`;
  jpegPctText.textContent = `${pct}%`;
  jpegMeterFill.style.width = `${pct}%`;
  if (len > PART_SIZE) {
    jpegMeterFill.classList.add("over");
  } else {
    jpegMeterFill.classList.remove("over");
  }
}

async function loadFile(file) {
  if (!file) return;
  fileLabel.textContent = file.name;

  const lower = file.name.toLowerCase();
  // Support opening an existing .img or bootlogo.bin/fex to extract & edit its JPEG
  if (lower.endsWith(".img") || lower.endsWith(".bin") || lower.endsWith(".fex")) {
    try {
      const buf = new Uint8Array(await file.arrayBuffer());
      const extractedJpg = callWasmExtractJpg(buf);
      const blob = new Blob([extractedJpg], { type: "image/jpeg" });
      const url = URL.createObjectURL(blob);
      const img = new Image();
      img.onload = () => {
        URL.revokeObjectURL(url);
        state.img = img;
        state.sourceName = file.name.replace(/\.[^.]+$/, "");
        sourceDimPill.textContent = `Extracted from ${file.name}: ${img.width}×${img.height} (${extractedJpg.length.toLocaleString()} B)`;
        resetTransform();
        renderAndMeasure();
        setStatus(
          `Extracted existing bootlogo JPEG (<strong>${img.width}×${img.height}</strong>, ${extractedJpg.length.toLocaleString()} bytes) from <code>${file.name}</code>.`,
          "ok"
        );
      };
      img.src = url;
    } catch (err) {
      setStatus(`Could not extract bootlogo from <code>${file.name}</code>: ${err.message}`, "err");
    }
    return;
  }

  const url = URL.createObjectURL(file);
  const img = new Image();
  img.onload = () => {
    URL.revokeObjectURL(url);
    state.img = img;
    state.sourceName = file.name.replace(/\.[^.]+$/, "");
    sourceDimPill.textContent = `Source: ${img.width}×${img.height} (${file.name})`;
    resetTransform();
    renderAndMeasure();
    setStatus(
      `Loaded <code>${file.name}</code> (<strong>${img.width}×${img.height}</strong>). Drag on the canvas to frame your <code>1024×600</code> bootlogo.`
    );
  };
  img.onerror = () => {
    URL.revokeObjectURL(url);
    setStatus(`Unsupported or corrupted image file: <code>${file.name}</code>`, "err");
  };
  img.src = url;
}

// Event Listeners: File Open & Drag-and-Drop
openBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  fileInput.click();
});

sampleBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  createDemoImage();
  setStatus("Loaded 1024×600 test pattern.");
});

dropzone.addEventListener("click", () => fileInput.click());

fileInput.addEventListener("change", () => {
  if (fileInput.files && fileInput.files[0]) {
    loadFile(fileInput.files[0]);
  }
});

["dragenter", "dragover"].forEach((evt) => {
  window.addEventListener(evt, (e) => {
    e.preventDefault();
    dropzone.classList.add("dragover");
  });
});

["dragleave", "drop"].forEach((evt) => {
  window.addEventListener(evt, (e) => {
    e.preventDefault();
    dropzone.classList.remove("dragover");
  });
});

window.addEventListener("drop", (e) => {
  if (e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0]) {
    loadFile(e.dataTransfer.files[0]);
  }
});

// Canvas Interactive Pan & Wheel Zoom
editorCanvas.addEventListener("pointerdown", (e) => {
  state.dragging = true;
  editorCanvas.setPointerCapture(e.pointerId);
  const rect = editorCanvas.getBoundingClientRect();
  const scaleX = TARGET_W / rect.width;
  const scaleY = TARGET_H / rect.height;
  state.dragStartX = e.clientX * scaleX - state.panX;
  state.dragStartY = e.clientY * scaleY - state.panY;
});

editorCanvas.addEventListener("pointermove", (e) => {
  if (!state.dragging) return;
  const rect = editorCanvas.getBoundingClientRect();
  const scaleX = TARGET_W / rect.width;
  const scaleY = TARGET_H / rect.height;
  state.panX = e.clientX * scaleX - state.dragStartX;
  state.panY = e.clientY * scaleY - state.dragStartY;
  renderAndMeasure();
});

["pointerup", "pointercancel"].forEach((evt) => {
  editorCanvas.addEventListener(evt, () => {
    state.dragging = false;
  });
});

editorCanvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const delta = e.deltaY < 0 ? 5 : -5;
    const nextPct = Math.min(400, Math.max(10, Number(zoomRange.value) + delta));
    zoomRange.value = nextPct;
    state.zoom = nextPct / 100;
    zoomValue.textContent = `${nextPct}%`;
    renderAndMeasure();
  },
  { passive: false }
);

// Mode Buttons
document.querySelectorAll("[data-mode]").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.querySelectorAll("[data-mode]").forEach((b) => b.classList.remove("active"));
    btn.classList.add("active");
    state.mode = btn.dataset.mode;
    modeLabel.textContent = btn.textContent;
    state.panX = 0;
    state.panY = 0;
    renderAndMeasure();
  });
});

// Zoom & Quality Sliders
zoomRange.addEventListener("input", () => {
  const pct = Number(zoomRange.value);
  state.zoom = pct / 100;
  zoomValue.textContent = `${pct}%`;
  renderAndMeasure();
});

qualityRange.addEventListener("input", () => {
  const q = Number(qualityRange.value);
  state.quality = q / 100;
  qualityValue.textContent = `${q}%`;
  renderAndMeasure();
});

// Orientation Buttons
rotLeftBtn.addEventListener("click", () => {
  state.rotation = (state.rotation - 90) % 360;
  renderAndMeasure();
});

rotRightBtn.addEventListener("click", () => {
  state.rotation = (state.rotation + 90) % 360;
  renderAndMeasure();
});

flipHBtn.addEventListener("click", () => {
  state.flipH = !state.flipH;
  renderAndMeasure();
});

flipVBtn.addEventListener("click", () => {
  state.flipV = !state.flipV;
  renderAndMeasure();
});

centerBtn.addEventListener("click", () => {
  resetTransform();
  renderAndMeasure();
});

bgColorInput.addEventListener("input", () => {
  state.bgColor = bgColorInput.value;
  bgColorHex.textContent = bgColorInput.value.toUpperCase();
  renderAndMeasure();
});

gridBtn.addEventListener("click", () => {
  state.showGrid = !state.showGrid;
  gridBtn.classList.toggle("active", state.showGrid);
  drawEditorCanvas();
});

function triggerDownload(bytes, filename, mime) {
  const blob = new Blob([bytes], { type: mime });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1500);
}

downloadJpgBtn.addEventListener("click", () => {
  renderAndMeasure();
  triggerDownload(state.lastJpegBytes, `${state.sourceName}_1024x600.jpg`, "image/jpeg");
});

generateImgBtn.addEventListener("click", () => {
  if (!state.wasm) return;
  try {
    drawToRenderCanvas();
    let q = state.quality;
    let jpgBytes = encodeBaselineJpeg(q);

    // Automatically step down quality if JPEG exceeds 128 KiB partition size
    while (jpgBytes.length > PART_SIZE && q > 0.35) {
      q = Math.max(0.35, +(q - 0.04).toFixed(2));
      jpgBytes = encodeBaselineJpeg(q);
    }
    if (q !== state.quality) {
      const pct = Math.round(q * 100);
      qualityRange.value = pct;
      qualityValue.textContent = `${pct}%`;
      state.quality = q;
      renderAndMeasure();
    }

    const imgBytes = callWasmBuildImg(jpgBytes);
    // Extract the 40-byte ASCII md5sum trailer at the end of the image
    const trailerBytes = imgBytes.slice(imgBytes.length - 40);
    const trailerStr = new TextDecoder().decode(trailerBytes).trim();

    let fname = (outFileName.value || "bootlogo_update.img").trim();
    if (!fname.toLowerCase().endsWith(".img")) {
      fname += ".img";
    }

    triggerDownload(imgBytes, fname, "application/octet-stream");
    setStatus(
      `✅ Built <code>${fname}</code> (<strong>${imgBytes.length.toLocaleString()} bytes</strong>)<br/>` +
        `<span class="mono">JPEG: 1024×600 (${jpgBytes.length.toLocaleString()} B, Q=${Math.round(q * 100)}%) • Trailer: ${trailerStr}</span>`,
      "ok"
    );
  } catch (err) {
    setStatus(`❌ Failed to build .img: ${err.message}`, "err");
  }
});

// Startup
createDemoImage();
initWasm();

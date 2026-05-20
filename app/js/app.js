import { computeDisplayHash, prepareImageForBackend } from "./phash.js";
import {
  connectWallet,
  getConnectedWallet,
  signAndSendTransaction,
} from "./wallet.js";

const cfg = window.BLINKPROOF_CONFIG ?? {};
const BLINK_ACTION_BASE = cfg.blinkActionBase ?? "http://localhost:3000";
const INDEXER_BASE      = cfg.indexerBase     ?? "http://localhost:3001";
const STATS_REFRESH_MS = 30_000;

const state = {
  file: null,
  previewUrl: null,
  wallet: null,
  pendingTransaction: null,
  i18n: window.__i18n || {},
};

const elements = {};

document.addEventListener("DOMContentLoaded", () => {
  bindElements();
  setupDropZone();
  setupWalletButton();
  setupVerifyButton();
  loadStats();
  window.setInterval(loadStats, STATS_REFRESH_MS);
});

// Listen for language changes from index.html
window.addEventListener("langChanged", (e) => {
  state.i18n = e.detail.i18n;
  if (state.wallet) updateWalletButton();
  if (elements.verifyButton && !elements.verifyButton.disabled) {
    elements.verifyButton.textContent = state.i18n['btn-verify'] || "Verify Provenance";
  }
  // Refresh stats to update time strings
  loadStats();
});

function bindElements() {
  Object.assign(elements, {
    walletButton: document.querySelector("#wallet-button"),
    dropZone: document.querySelector("#drop-zone"),
    fileInput: document.querySelector("#file-input"),
    chooseFileButton: document.querySelector("#choose-file-button"),
    previewContainer: document.querySelector("#preview-container"),
    previewImage: document.querySelector("#preview-image"),
    phashInfo: document.querySelector("#phash-info"),
    displayHash: document.querySelector("#display-hash"),
    fileMeta: document.querySelector("#file-meta"),
    verifyButton: document.querySelector("#verify-button"),
    resultSection: document.querySelector("#result-section"),
    resultCard: document.querySelector("#result-card"),
    resultIcon: document.querySelector("#result-icon"),
    resultLabel: document.querySelector("#result-label"),
    phashLabel: document.querySelector("#phash-label"),
    resultTitle: document.querySelector("#result-title"),
    resultMessage: document.querySelector("#result-message"),
    resultActions: document.querySelector("#result-actions"),
    heroStatTotal: document.querySelector("#hero-stat-total"),
    statTotal: document.querySelector("#stat-total"),
    statCreators: document.querySelector("#stat-creators"),
    statLatestHash: document.querySelector("#stat-latest-hash"),
    statLatestTime: document.querySelector("#stat-latest-time"),
  });
}

function setupDropZone() {
  elements.chooseFileButton.addEventListener("click", () => elements.fileInput.click());
  elements.dropZone.addEventListener("click", (event) => {
    if (event.target !== elements.chooseFileButton) {
      elements.fileInput.click();
    }
  });
  elements.dropZone.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      elements.fileInput.click();
    }
  });
  elements.fileInput.addEventListener("change", () => {
    const file = elements.fileInput.files?.[0];
    if (file) handleImageFile(file);
  });

  for (const eventName of ["dragenter", "dragover"]) {
    elements.dropZone.addEventListener(eventName, (event) => {
      event.preventDefault();
      elements.dropZone.classList.add("is-dragging");
    });
  }

  for (const eventName of ["dragleave", "drop"]) {
    elements.dropZone.addEventListener(eventName, (event) => {
      event.preventDefault();
      elements.dropZone.classList.remove("is-dragging");
    });
  }

  elements.dropZone.addEventListener("drop", (event) => {
    const file = Array.from(event.dataTransfer?.files ?? []).find((item) =>
      item.type.startsWith("image/"),
    );
    if (file) handleImageFile(file);
  });

  document.addEventListener("paste", (event) => {
    const file = Array.from(event.clipboardData?.files ?? []).find((item) =>
      item.type.startsWith("image/"),
    );
    if (file) handleImageFile(file);
  });
}

function setupWalletButton() {
  state.wallet = getConnectedWallet();
  updateWalletButton();

  elements.walletButton.addEventListener("click", async () => {
    try {
      state.wallet = await connectWallet();
      updateWalletButton();
    } catch (error) {
      showResult("warning", "⚠️", state.i18n['wallet-error-title'] || "Wallet Error", error.message);
    }
  });
}

function setupVerifyButton() {
  elements.verifyButton.addEventListener("click", verifyCurrentImage);
}

async function handleImageFile(file) {
  if (!file.type.startsWith("image/")) {
    showResult("warning", "⚠️", state.i18n['file-error-title'] || "Unsupported File", state.i18n['file-error-msg'] || "Please select an image file.");
    return;
  }

  state.file = file;
  state.pendingTransaction = null;

  if (state.previewUrl) URL.revokeObjectURL(state.previewUrl);
  state.previewUrl = URL.createObjectURL(file);
  elements.previewImage.src = state.previewUrl;
  elements.previewContainer.hidden = false;
  elements.phashInfo.hidden = false;
  elements.displayHash.textContent = state.i18n['calculating'] || "Calculating...";
  elements.fileMeta.textContent = `${file.name} · ${formatBytes(file.size)}`;

  try {
    const { displayHash } = await computeDisplayHash(file);
    elements.displayHash.textContent = displayHash;
  } catch (error) {
    elements.displayHash.textContent = "Hash failed";
    elements.fileMeta.textContent = error.message;
  }
}

async function verifyCurrentImage() {
  if (!state.file) {
    showResult("warning", "⚠️", state.i18n['no-image-title'] || "No Image", state.i18n['no-image-msg'] || "Please upload an image first.");
    return;
  }

  state.wallet = getConnectedWallet() || state.wallet;
  if (!state.wallet) {
    try {
      state.wallet = await connectWallet();
      updateWalletButton();
    } catch (error) {
      showResult("warning", "⚠️", "钱包错误", "请先连接钱包后再进行存证验证！");
      return;
    }
  }

  setBusy(true, state.i18n['msg-request-action'] || "Requesting Action...");

  try {
    const imageUrl = await prepareImageForBackend(state.file);
    const response = await fetch(`${BLINK_ACTION_BASE}/api/actions/verify`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ account: state.wallet, image_url: imageUrl }),
    });
    const payload = await response.json().catch(() => ({}));

    if (!response.ok) {
      throw new Error(payload.message ?? "Blink Action failed");
    }

    if (!payload.transaction) {
      renderVerificationMessage(payload.message ?? "Verification complete.");
      loadStats();
      return;
    }

    state.pendingTransaction = payload.transaction;
    showResult("action", "🔗", state.i18n['not-found-title'] || "Not Found", payload.message, [
      actionButton(state.i18n['btn-sign'] || "Sign Attestation", signPendingTransaction),
    ]);
  } catch (error) {
    showResult("error", "⛔", state.i18n['verify-failed-title'] || "Verify Failed", error.message);
  } finally {
    setBusy(false);
  }
}

async function signPendingTransaction() {
  if (!state.pendingTransaction) return;

  setBusy(true, state.i18n['msg-waiting-sign'] || "Waiting for signature...");

  try {
    const signature = await signAndSendTransaction(state.pendingTransaction);
    const cluster = cfg.solanaCluster ?? "custom";
    const rpcParam = cluster === "custom"
      ? `&customUrl=${encodeURIComponent(cfg.solanaRpcUrl ?? "http://127.0.0.1:8899")}`
      : "";
    const link = `https://explorer.solana.com/tx/${signature}?cluster=${cluster}${rpcParam}`;
    showResult("success", "✅", state.i18n['tx-submitted-title'] || "Transaction Submitted", `Signature: ${signature}`, [
      evidenceLink(state.i18n['btn-view-evidence'] || "View Evidence", link),
    ]);
    state.pendingTransaction = null;
    setTimeout(loadStats, 1500);
  } catch (error) {
    showResult("error", "⛔", state.i18n['sign-failed-title'] || "Sign Failed", error.message);
  } finally {
    setBusy(false);
  }
}

async function loadStats() {
  try {
    const response = await fetch(`${INDEXER_BASE}/stats`, { cache: "no-store" });
    if (!response.ok) throw new Error("Indexer 统计接口不可用");
    const stats = await response.json();
    const latest = stats.latest_registration;

    if (elements.heroStatTotal) elements.heroStatTotal.textContent = formatNumber(stats.total_fingerprints);
    elements.statTotal.textContent = formatNumber(stats.total_fingerprints);
    elements.statCreators.textContent = formatNumber(stats.unique_creators_count);
    elements.statLatestHash.textContent = latest?.hash_prefix ?? "--";
    elements.statLatestTime.textContent = latest?.timestamp
      ? `${state.i18n['stat-recently'] || "Recently Anchored"}: ${latest.timestamp}`
      : (state.i18n['stat-no-recent'] || "No recent attestations");
  } catch (error) {
    if (elements.heroStatTotal) elements.heroStatTotal.textContent = "OFFLINE";
    elements.statTotal.textContent = "--";
    elements.statCreators.textContent = "--";
    elements.statLatestHash.textContent = "--";
    elements.statLatestTime.textContent = error.message;
  }
}

function renderVerificationMessage(message) {
  if (message.includes("⚠️")) {
    showResult("warning", "⚠️", state.i18n['similarity-alert-title'] || "SIMILARITY ALERT", message);
    return;
  }

  showResult("success", "✅", state.i18n['verified-original-title'] || "VERIFIED ORIGINAL", message);
}

function showResult(type, icon, title, message, actions = []) {
  elements.resultSection.hidden = false;
  elements.resultCard.className = `result-card is-${type}`;
  elements.resultIcon.textContent = icon;
  elements.resultLabel.className = `status-pill status-${type}`;
  elements.resultLabel.textContent = typeLabel(type);
  elements.resultTitle.textContent = title;
  elements.resultMessage.textContent = message;
  elements.resultActions.replaceChildren(...actions);
}

function actionButton(label, onClick) {
  const button = document.createElement("button");
  button.className = "button button-accent";
  button.type = "button";
  button.textContent = label;
  button.addEventListener("click", onClick);
  return button;
}

function evidenceLink(label, href) {
  const link = document.createElement("a");
  link.className = "evidence-link";
  link.href = href;
  link.target = "_blank";
  link.rel = "noreferrer";
  link.textContent = label;
  return link;
}

function setBusy(isBusy, label = (state.i18n['btn-verify'] || "Verify Provenance")) {
  elements.verifyButton.disabled = isBusy;
  elements.verifyButton.textContent = isBusy ? label : (state.i18n['btn-verify'] || "Verify Provenance");
}

function updateWalletButton() {
  const wallet = state.wallet ?? getConnectedWallet();
  elements.walletButton.textContent = wallet ? shortAddress(wallet) : (state.i18n['wallet-connect'] || "Connect Wallet");
}

function typeLabel(type) {
  const labels = {
    success: state.i18n['label-verified'] || "Verified",
    warning: state.i18n['label-similarity'] || "Similarity alert",
    action: state.i18n['label-action'] || "Action required",
    error: state.i18n['label-error'] || "Error",
  };
  return labels[type] ?? "Status";
}

function shortAddress(address) {
  return `${address.slice(0, 4)}...${address.slice(-4)}`;
}

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

function formatNumber(value) {
  const locale = document.documentElement.lang === 'zh' ? 'zh-CN' : 'en-US';
  return new Intl.NumberFormat(locale).format(Number(value ?? 0));
}

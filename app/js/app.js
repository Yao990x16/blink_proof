import { computeDisplayHash, prepareImageForBackend } from "./phash.js";
import {
  connectWallet,
  getConnectedWallet,
  signAndSendTransaction,
} from "./wallet.js";

const BLINK_ACTION_BASE = "http://localhost:3000";
const INDEXER_BASE = "http://localhost:3001";
const STATS_REFRESH_MS = 30_000;

const state = {
  file: null,
  previewUrl: null,
  wallet: null,
  pendingTransaction: null,
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

function bindElements() {
  Object.assign(elements, {
    walletButton: document.querySelector("#wallet-button"),
    dropZone: document.querySelector("#drop-zone"),
    fileInput: document.querySelector("#file-input"),
    chooseFileButton: document.querySelector("#choose-file-button"),
    previewContainer: document.querySelector("#preview-container"),
    previewImage: document.querySelector("#preview-image"),
    displayHash: document.querySelector("#display-hash"),
    fileMeta: document.querySelector("#file-meta"),
    verifyButton: document.querySelector("#verify-button"),
    resultSection: document.querySelector("#result-section"),
    resultCard: document.querySelector("#result-card"),
    resultIcon: document.querySelector("#result-icon"),
    resultLabel: document.querySelector("#result-label"),
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
      showResult("warning", "⚠️", "钱包未连接", error.message);
    }
  });
}

function setupVerifyButton() {
  elements.verifyButton.addEventListener("click", verifyCurrentImage);
}

async function handleImageFile(file) {
  if (!file.type.startsWith("image/")) {
    showResult("warning", "⚠️", "文件类型不支持", "请选择 PNG、JPEG、WebP 等图片文件。");
    return;
  }

  state.file = file;
  state.pendingTransaction = null;

  if (state.previewUrl) URL.revokeObjectURL(state.previewUrl);
  state.previewUrl = URL.createObjectURL(file);
  elements.previewImage.src = state.previewUrl;
  elements.previewContainer.hidden = false;
  elements.displayHash.textContent = "计算中...";
  elements.fileMeta.textContent = `${file.name} · ${formatBytes(file.size)}`;

  try {
    const { displayHash } = await computeDisplayHash(file);
    elements.displayHash.textContent = displayHash;
  } catch (error) {
    elements.displayHash.textContent = "前端近似 hash 计算失败";
    elements.fileMeta.textContent = error.message;
  }
}

async function verifyCurrentImage() {
  if (!state.file) {
    showResult("warning", "⚠️", "还没有图片", "请先上传或粘贴一张图片。");
    return;
  }

  state.wallet = getConnectedWallet() || state.wallet;
  if (!state.wallet) {
    try {
      state.wallet = await connectWallet();
      updateWalletButton();
    } catch (error) {
      showResult("warning", "⚠️", "需要连接钱包", error.message);
      return;
    }
  }

  setBusy(true, "正在请求 Blink Action...");

  try {
    const imageUrl = await prepareImageForBackend(state.file);
    const response = await fetch(`${BLINK_ACTION_BASE}/api/actions/verify`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ account: state.wallet, image_url: imageUrl }),
    });
    const payload = await response.json().catch(() => ({}));

    if (!response.ok) {
      throw new Error(payload.message ?? "Blink Action 请求失败");
    }

    if (!payload.transaction) {
      renderVerificationMessage(payload.message ?? "该内容已完成核验。 ");
      loadStats();
      return;
    }

    state.pendingTransaction = payload.transaction;
    showResult("action", "🔗", "未发现链上存证", payload.message, [
      actionButton("签名存证", signPendingTransaction),
    ]);
  } catch (error) {
    showResult("error", "⛔", "验证失败", error.message);
  } finally {
    setBusy(false);
  }
}

async function signPendingTransaction() {
  if (!state.pendingTransaction) return;

  setBusy(true, "等待钱包签名...");

  try {
    const signature = await signAndSendTransaction(state.pendingTransaction);
    const link = `https://explorer.solana.com/tx/${signature}?cluster=custom&customUrl=http%3A%2F%2F127.0.0.1%3A8899`;
    showResult("success", "✅", "交易已提交", `签名：${signature}`, [
      evidenceLink("查看链上证据", link),
    ]);
    state.pendingTransaction = null;
    setTimeout(loadStats, 1500);
  } catch (error) {
    showResult("error", "⛔", "签名或发送失败", error.message);
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

    elements.heroStatTotal.textContent = formatNumber(stats.total_fingerprints);
    elements.statTotal.textContent = formatNumber(stats.total_fingerprints);
    elements.statCreators.textContent = formatNumber(stats.unique_creators_count);
    elements.statLatestHash.textContent = latest?.hash_prefix ?? "--";
    elements.statLatestTime.textContent = latest?.timestamp
      ? `最近存证时间：${latest.timestamp}`
      : "暂无最近存证";
  } catch (error) {
    elements.heroStatTotal.textContent = "离线";
    elements.statTotal.textContent = "--";
    elements.statCreators.textContent = "--";
    elements.statLatestHash.textContent = "--";
    elements.statLatestTime.textContent = error.message;
  }
}

function renderVerificationMessage(message) {
  if (message.includes("⚠️")) {
    showResult("warning", "⚠️", "发现高度相似内容", message);
    return;
  }

  showResult("success", "✅", "官方原图存证", message);
}

function showResult(type, icon, title, message, actions = []) {
  elements.resultSection.hidden = false;
  elements.resultCard.className = `result-card is-${type}`;
  elements.resultIcon.textContent = icon;
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

function setBusy(isBusy, label = "开始验证") {
  elements.verifyButton.disabled = isBusy;
  elements.verifyButton.textContent = isBusy ? label : "开始验证";
}

function updateWalletButton() {
  const wallet = state.wallet ?? getConnectedWallet();
  elements.walletButton.textContent = wallet ? shortAddress(wallet) : "连接钱包";
}

function typeLabel(type) {
  return {
    success: "Verified",
    warning: "Similarity alert",
    action: "Action required",
    error: "Error",
  }[type] ?? "Status";
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
  return new Intl.NumberFormat("zh-CN").format(Number(value ?? 0));
}

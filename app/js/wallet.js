/**
 * 连接 Phantom 钱包。
 * @returns {Promise<string>} 连接的公钥（base58）
 */
export async function connectWallet() {
  if (!window.solana?.isPhantom) {
    throw new Error("请安装 Phantom 钱包");
  }

  const response = await window.solana.connect();
  return response.publicKey.toString();
}

/**
 * 获取当前连接的钱包公钥。
 * @returns {string|null}
 */
export function getConnectedWallet() {
  return window.solana?.publicKey?.toString() || null;
}

/**
 * 签名并发送由后端返回的 base64 序列化交易。
 * @param {string} base64Transaction - base64 编码的未签名交易
 * @returns {Promise<string>} 交易签名
 */
export async function signAndSendTransaction(base64Transaction) {
  if (!window.solana?.isPhantom) {
    throw new Error("请安装 Phantom 钱包");
  }

  const bytes = Uint8Array.from(atob(base64Transaction), (char) =>
    char.charCodeAt(0),
  );
  const transaction = window.solanaWeb3.Transaction.from(bytes);
  const result = await window.solana.signAndSendTransaction(transaction);

  return typeof result === "string" ? result : result.signature;
}

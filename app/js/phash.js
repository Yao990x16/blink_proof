/**
 * 使用 Canvas API 计算图片的近似感知哈希（仅用于前端 UI 展示）。
 * 实际存证和验证由后端 Blink Action 服务计算精确哈希。
 *
 * @param {File|Blob} imageFile
 * @returns {Promise<{displayHash: string}>}
 */
export async function computeDisplayHash(imageFile) {
  const image = await loadImage(imageFile);
  const canvas = document.createElement("canvas");
  canvas.width = 9;
  canvas.height = 8;

  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) {
    throw new Error("当前浏览器不支持 Canvas 2D 上下文");
  }

  context.drawImage(image, 0, 0, canvas.width, canvas.height);
  const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
  const gray = [];

  for (let index = 0; index < pixels.length; index += 4) {
    const red = pixels[index];
    const green = pixels[index + 1];
    const blue = pixels[index + 2];
    gray.push(Math.round(red * 0.299 + green * 0.587 + blue * 0.114));
  }

  const bytes = new Uint8Array(8);
  for (let y = 0; y < 8; y += 1) {
    let byte = 0;
    for (let x = 0; x < 8; x += 1) {
      const left = gray[y * 9 + x];
      const right = gray[y * 9 + x + 1];
      if (left > right) {
        byte |= 1 << (7 - x);
      }
    }
    bytes[y] = byte;
  }

  return { displayHash: toHex(bytes) };
}

/**
 * 将图片文件转为可供后端下载的临时 Object URL 或 base64。
 *
 * @param {File|Blob} imageFile
 * @returns {Promise<string>} 可用于 POST 请求的图片引用
 */
export async function prepareImageForBackend(imageFile) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(reader.error ?? new Error("图片读取失败"));
    reader.readAsDataURL(imageFile);
  });
}

function loadImage(imageFile) {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(imageFile);
    const image = new Image();
    image.onload = () => {
      URL.revokeObjectURL(url);
      resolve(image);
    };
    image.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("图片解码失败"));
    };
    image.src = url;
  });
}

function toHex(bytes) {
  return Array.from(bytes)
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

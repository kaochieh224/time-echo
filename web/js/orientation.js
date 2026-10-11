// 來源方向檢查：有些瀏覽器（iPadOS／iOS 的 WebKit）把 video 直接複製進 GPU 貼圖時會上下顛倒。
// 做法是把 GPU 貼圖讀回幾小塊，和 2D canvas 畫出來的同一張影格比對（2D canvas 的方向一定是對的）。

/** 比對用的小塊：寬 64（讀回時每列剛好 256 bytes）、高 16，上緣與下緣各取左、中、右三塊。 */
export const BLOCK_W = 64;
export const BLOCK_H = 16;

/** 回傳 [上緣三塊, 下緣三塊] 的左上角座標；影格太小時回傳 null。 */
export function probeBlocks(width, height) {
    if (width < BLOCK_W * 3 || height < BLOCK_H * 4) return null;
    const xs = [0, (width - BLOCK_W) >> 1, width - BLOCK_W];
    return [xs.map((x) => [x, 0]), xs.map((x) => [x, height - BLOCK_H])];
}

/** 一塊 RGBA8 的平均顏色 [r, g, b]。`stride` 是每列的 byte 數。 */
export function blockMean(rgba, offset = 0, stride = BLOCK_W * 4) {
    const sum = [0, 0, 0];
    for (let y = 0; y < BLOCK_H; y++) {
        for (let x = 0; x < BLOCK_W; x++) {
            const i = offset + y * stride + x * 4;
            sum[0] += rgba[i];
            sum[1] += rgba[i + 1];
            sum[2] += rgba[i + 2];
        }
    }
    return sum.map((v) => v / (BLOCK_W * BLOCK_H));
}

const distance = (a, b) => a.flat().reduce((acc, v, i) => acc + Math.abs(v - b.flat()[i]), 0);

/**
 * 比較 GPU 貼圖與參考畫面的上、下緣平均顏色（各三塊）。
 * 回傳 'ok'（方向一致）、'flipped'（上下顛倒）或 'unsure'（畫面上下太像，分不出來）。
 */
export function compareOrientation(gpu, ref) {
    const [gTop, gBottom] = gpu;
    const [rTop, rBottom] = ref;
    // 參考畫面的上下緣差太少（例如全黑）就不判斷，等下一張
    if (distance(rTop, rBottom) < 9 * 6) return 'unsure';
    const same = distance(gTop, rTop) + distance(gBottom, rBottom);
    const swapped = distance(gTop, rBottom) + distance(gBottom, rTop);
    if (Math.min(same, swapped) > 0.5 * Math.max(same, swapped)) return 'unsure';
    return swapped < same ? 'flipped' : 'ok';
}

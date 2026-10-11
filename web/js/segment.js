// AI 人物分割：MediaPipe Image Segmenter 的自拍分割模型（Apache 2.0，約 250 KB）。
// 桌面版用的 RVM 是 GPL-3.0，而且在手機瀏覽器上太重，所以網頁版換成這個。
// 函式庫與模型在第一次用到 AI 遮罩時才從 CDN 下載。

const VERSION = '1.1.0';
const PACKAGE = `https://cdn.jsdelivr.net/npm/@mediapipe/tasks-vision@${VERSION}`;
const MODEL = 'https://storage.googleapis.com/mediapipe-models/image_segmenter/selfie_segmenter/float16/latest/selfie_segmenter.tflite';

/**
 * WebKit（Safari，以及 iPadOS／iOS 上的所有瀏覽器）用 GPU 跑模型時，遮罩讀回來是壞的
 * （和人對不起來），所以那邊優先用 CPU。Chrome、Edge 這些 Blink 瀏覽器用 GPU 沒問題。
 */
export function preferredDelegates(userAgent) {
    const blink = /Chrome\/|Chromium\/|Edg\//.test(userAgent);
    return blink ? ['GPU', 'CPU'] : ['CPU', 'GPU'];
}

export class PersonSegmenter {
    constructor(segmenter, delegate) {
        this.segmenter = segmenter;
        this.delegate = delegate;
        this.lastTimestamp = 0;
    }

    /** 下載函式庫與模型並建立分割器；偏好的執行方式不能用時退回另一種。 */
    static async create() {
        const { FilesetResolver, ImageSegmenter } = await import(`${PACKAGE}/vision_bundle.mjs`);
        const fileset = await FilesetResolver.forVisionTasks(`${PACKAGE}/wasm`);
        let lastError;
        for (const delegate of preferredDelegates(navigator.userAgent)) {
            try {
                const segmenter = await ImageSegmenter.createFromOptions(fileset, {
                    baseOptions: { modelAssetPath: MODEL, delegate },
                    runningMode: 'VIDEO',
                    outputConfidenceMasks: true,
                    outputCategoryMask: false,
                });
                return new PersonSegmenter(segmenter, delegate);
            } catch (e) {
                lastError = e;
            }
        }
        throw lastError;
    }

    /**
     * 對一張影格做分割，回傳 { data, width, height }：模型解析度的人物機率（0–1，1 = 人物）。
     * 同步執行。`image` 用 ImageData：直接給畫素，不會有 canvas／video 傳進 GPU 時方向不一致的問題。
     */
    segment(image, nowMs) {
        // MediaPipe 要求時間戳嚴格遞增
        this.lastTimestamp = Math.max(this.lastTimestamp + 1, Math.round(nowMs));
        let out = null;
        this.segmenter.segmentForVideo(image, this.lastTimestamp, (result) => {
            // 遮罩只在這個 callback 裡有效，所以複製出來。
            // 自拍模型只有一張信心圖（人物）；多類別模型的第 0 張是背景，取最後一張。
            const mask = result.confidenceMasks.at(-1);
            out = { data: new Float32Array(mask.getAsFloat32Array()), width: mask.width, height: mask.height };
        });
        if (!out) throw new Error('分割沒有回傳遮罩');
        return out;
    }
}

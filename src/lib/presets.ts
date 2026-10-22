// Timeline presets shown on the Project Hub and in the New Project dialog.

export interface Preset {
  id: string;
  name: string;
  use: string;
  width: number;
  height: number;
  num: number;
  den: number;
}

export const PRESETS: Preset[] = [
  { id: "hd-1080-25", name: "1080p HD Landscape", use: "16:9 · broadcast, web", width: 1920, height: 1080, num: 25, den: 1 },
  { id: "uhd-4k-24", name: "4K UHD Mastering", use: "16:9 · film look, finishing", width: 3840, height: 2160, num: 24, den: 1 },
  { id: "vertical-1080", name: "Vertical Social / Reels", use: "9:16 · Reels, TikTok, Shorts", width: 1080, height: 1920, num: 30, den: 1 },
];

export const RATES: [string, number, number][] = [
  ["23.976", 24000, 1001], ["24", 24, 1], ["25", 25, 1], ["29.97", 30000, 1001], ["30", 30, 1], ["50", 50, 1], ["59.94", 60000, 1001], ["60", 60, 1],
];

export const fpsLabel = (num: number, den: number) => (den ? (num / den).toFixed(3).replace(/\.?0+$/, "") : "?");

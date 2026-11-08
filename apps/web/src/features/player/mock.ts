/**
 * 小窗原型的假数据（T-0004，不接 IPC）。
 * 中英文标题各留一组超长文本，用来验证单行省略。
 */
export type MockTrack = {
  title: string;
  artist: string;
  album: string;
  coverSeed: string;
  durationMs: number;
  positionMs: number;
};

export const MOCK_TRACKS: readonly MockTrack[] = [
  {
    title: "The Less I Know the Better (Slowed + Reverb)",
    artist: "Tame Impala",
    album: "Currents",
    coverSeed: "currents",
    durationMs: 216_000,
    positionMs: 42_000,
  },
  {
    title: "我用什么把你留住（现场版）",
    artist: "声音玩具",
    album: "劳动之余",
    coverSeed: "laodong",
    durationMs: 245_000,
    positionMs: 83_000,
  },
  {
    title: "晴天",
    artist: "周杰伦",
    album: "叶惠美",
    coverSeed: "yehuimei",
    durationMs: 269_000,
    positionMs: 128_000,
  },
];

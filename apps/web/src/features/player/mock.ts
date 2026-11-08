/**
 * 小窗原型的假数据（T-0004，不接 IPC）。
 * 中英文标题各留一组超长文本，用来验证单行省略。
 */
import type { PlayerState } from "./state";

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
    title: "我用什么把你留住（2024 巡演现场版）",
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

const IDLE_TRACK: MockTrack = {
  title: "未在播放",
  artist: "Nowhere Player",
  album: "Nowhere",
  coverSeed: "nowhere-idle",
  durationMs: 0,
  positionMs: 0,
};

/** 加载中尚未开始播放，进度必须是 0。 */
const LOADING_TRACK: MockTrack = {
  ...MOCK_TRACKS[2],
  positionMs: 0,
};

const TRACK_BY_STATE: Record<PlayerState, MockTrack> = {
  idle: IDLE_TRACK,
  loading: LOADING_TRACK,
  playing: MOCK_TRACKS[0],
  paused: MOCK_TRACKS[1],
  error: MOCK_TRACKS[0],
  hover: MOCK_TRACKS[0],
};

/** 6 种预览状态各配一组假数据，切状态时标题/封面/时间都会变。 */
export function mockForState(state: PlayerState): MockTrack {
  return TRACK_BY_STATE[state];
}

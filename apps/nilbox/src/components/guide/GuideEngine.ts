export type BubblePosition = "top" | "bottom" | "left" | "right" | "auto";

// "hello" 또는 { "en": "hello", "ko": "안녕" } 둘 다 허용
export type LocalizedString = string | Record<string, string>;

export function resolveLocale(val: LocalizedString, lang: string): string {
  if (typeof val === "string") return val;
  return val[lang] ?? val[lang.split("-")[0]] ?? val["en"] ?? Object.values(val)[0] ?? "";
}

export interface BubbleConfig {
  title: LocalizedString;
  description: LocalizedString;
  position: BubblePosition;
}

export type ExpectedAction =
  | { type: "click"; selector: string }
  | { type: "input"; selector: string; value?: string }
  | { type: "navigate"; screen: string }
  | { type: "custom"; eventName: string };

export type SkipCondition =
  | { type: "vmStatus"; value: string }
  | { type: "localStorage"; key: string; value: string };

export interface GuideStep {
  id: string;
  selector: string;
  navigateTo: string | null;
  bubble: BubbleConfig;
  expectedAction: ExpectedAction;
  skipIf: SkipCondition | null;
}

export interface GuideScenario {
  id: string;
  version: number;
  title: LocalizedString;
  steps: GuideStep[];
}

export interface ScenarioMeta {
  id: string;
  title: LocalizedString;
  description: LocalizedString;
  estimatedMinutes: number;
  tags: string[];
  file: string;
}

export interface ScenarioIndex {
  version: number;
  scenarios: ScenarioMeta[];
}

export type GuideState =
  | { status: "idle" }
  | { status: "loading"; scenarioId: string }
  | { status: "running"; scenario: GuideScenario; stepIndex: number }
  | { status: "validating"; scenario: GuideScenario; stepIndex: number }
  | { status: "paused"; scenario: GuideScenario; stepIndex: number }
  | { status: "completed"; scenario: GuideScenario };

export type GuideAction =
  | { type: "LOAD_START"; scenarioId: string }
  | { type: "LOAD_DONE"; scenario: GuideScenario }
  | { type: "LOAD_ERROR" }
  | { type: "ADVANCE" }
  | { type: "SKIP_STEP" }
  | { type: "PAUSE" }
  | { type: "RESUME" }
  | { type: "EXIT" }
  | { type: "ACTION_REPORTED"; eventType: string; selector: string; value?: string };

export function guideReducer(state: GuideState, action: GuideAction): GuideState {
  switch (action.type) {
    case "LOAD_START":
      return { status: "loading", scenarioId: action.scenarioId };

    case "LOAD_DONE":
      return { status: "running", scenario: action.scenario, stepIndex: 0 };

    case "LOAD_ERROR":
      return { status: "idle" };

    case "ACTION_REPORTED": {
      if (state.status !== "running") return state;
      const step = state.scenario.steps[state.stepIndex];
      const ea = step.expectedAction;
      if (ea.type === "click" && action.eventType === "click") return { status: "validating", scenario: state.scenario, stepIndex: state.stepIndex };
      if (ea.type === "input" && action.eventType === "input") {
        if (!ea.value || ea.value === action.value) return { status: "validating", scenario: state.scenario, stepIndex: state.stepIndex };
      }
      return state;
    }

    case "ADVANCE": {
      if (state.status === "validating" || state.status === "running") {
        const nextIndex = state.stepIndex + 1;
        if (nextIndex >= state.scenario.steps.length) return { status: "completed", scenario: state.scenario };
        return { status: "running", scenario: state.scenario, stepIndex: nextIndex };
      }
      return state;
    }

    case "SKIP_STEP": {
      if (state.status === "running" || state.status === "validating") {
        const nextIndex = state.stepIndex + 1;
        if (nextIndex >= state.scenario.steps.length) return { status: "completed", scenario: state.scenario };
        return { status: "running", scenario: state.scenario, stepIndex: nextIndex };
      }
      return state;
    }

    case "PAUSE": {
      if (state.status === "running") return { status: "paused", scenario: state.scenario, stepIndex: state.stepIndex };
      return state;
    }

    case "RESUME": {
      if (state.status === "paused") return { status: "running", scenario: state.scenario, stepIndex: state.stepIndex };
      return state;
    }

    case "EXIT":
      return { status: "idle" };

    default:
      return state;
  }
}

export function getCurrentStep(state: GuideState): GuideStep | null {
  if (state.status === "running" || state.status === "validating" || state.status === "paused") {
    return state.scenario.steps[state.stepIndex] ?? null;
  }
  return null;
}

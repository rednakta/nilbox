import React, { createContext, useContext, useReducer, useCallback, useRef, useEffect } from "react";
import { GuideState, GuideScenario, ScenarioIndex, guideReducer } from "./GuideEngine";

interface GuideContextValue {
  state: GuideState;
  startScenario: (scenarioId: string) => Promise<void>;
  advance: () => void;
  skipStep: () => void;
  pause: () => void;
  resume: () => void;
  exit: () => void;
  reportAction: (eventType: string, selector: string, value?: string) => void;
  setActiveScreen: ((screen: string) => void) | null;
}

const GuideContext = createContext<GuideContextValue | null>(null);

interface GuideProviderProps {
  children: React.ReactNode;
  setActiveScreen: (screen: string) => void;
}

export const GuideProvider: React.FC<GuideProviderProps> = ({ children, setActiveScreen }) => {
  const [state, dispatch] = useReducer(guideReducer, { status: "idle" });
  const validatingTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Auto-advance after validating delay
  useEffect(() => {
    if (state.status === "validating") {
      validatingTimer.current = setTimeout(() => {
        dispatch({ type: "ADVANCE" });
      }, 300);
    }
    return () => {
      if (validatingTimer.current) clearTimeout(validatingTimer.current);
    };
  }, [state.status, state.status === "validating" ? (state as any).stepIndex : -1]);

  // Navigate to required screen when step changes
  useEffect(() => {
    if (state.status === "running") {
      const step = state.scenario.steps[state.stepIndex];
      if (step?.navigateTo) {
        setActiveScreen(step.navigateTo);
      }
    }
  }, [state.status === "running" ? state.stepIndex : -1]);

  const startScenario = useCallback(async (scenarioId: string) => {
    dispatch({ type: "LOAD_START", scenarioId });
    try {
      const indexResp = await fetch("/guides/index.json");
      const index: ScenarioIndex = await indexResp.json();
      const meta = index.scenarios.find((s) => s.id === scenarioId);
      if (!meta) throw new Error("Scenario not found");
      const resp = await fetch(meta.file);
      const scenario: GuideScenario = await resp.json();
      dispatch({ type: "LOAD_DONE", scenario });
    } catch {
      dispatch({ type: "LOAD_ERROR" });
    }
  }, []);

  const advance = useCallback(() => dispatch({ type: "ADVANCE" }), []);
  const skipStep = useCallback(() => dispatch({ type: "SKIP_STEP" }), []);
  const pause = useCallback(() => dispatch({ type: "PAUSE" }), []);
  const resume = useCallback(() => dispatch({ type: "RESUME" }), []);
  const exit = useCallback(() => dispatch({ type: "EXIT" }), []);
  const reportAction = useCallback((eventType: string, selector: string, value?: string) => {
    dispatch({ type: "ACTION_REPORTED", eventType, selector, value });
  }, []);

  return (
    <GuideContext.Provider value={{ state, startScenario, advance, skipStep, pause, resume, exit, reportAction, setActiveScreen }}>
      {children}
    </GuideContext.Provider>
  );
};

export function useGuide(): GuideContextValue {
  const ctx = useContext(GuideContext);
  if (!ctx) throw new Error("useGuide must be used within GuideProvider");
  return ctx;
}

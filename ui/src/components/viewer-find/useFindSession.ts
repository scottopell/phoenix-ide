import { useCallback, useEffect, useReducer, useRef } from 'react';
import {
  createClosedFindSession,
  reduceFindSession,
  type FindSessionAction,
  type FindSessionCommand,
  type FindSessionState,
} from './findSession';

interface HookState<TTarget, TFocusOrigin> {
  session: FindSessionState<TTarget, TFocusOrigin>;
  commands: readonly FindSessionCommand<TTarget, TFocusOrigin>[];
  commandVersion: number;
}

type CommandHandler<TTarget, TFocusOrigin> = (
  commands: readonly FindSessionCommand<TTarget, TFocusOrigin>[],
) => void;

export interface UseFindSessionOptions<TTarget, TFocusOrigin> {
  onCommands: CommandHandler<TTarget, TFocusOrigin>;
}

function createHookState<TTarget, TFocusOrigin>(): HookState<TTarget, TFocusOrigin> {
  return {
    session: createClosedFindSession(),
    commands: [],
    commandVersion: 0,
  };
}

function hookReducer<TTarget, TFocusOrigin>(
  state: HookState<TTarget, TFocusOrigin>,
  action: FindSessionAction<TTarget, TFocusOrigin>,
): HookState<TTarget, TFocusOrigin> {
  const result = reduceFindSession(state.session, action);
  return {
    session: result.state,
    commands: result.commands,
    commandVersion: result.commands.length > 0 ? state.commandVersion + 1 : state.commandVersion,
  };
}

export function useFindSession<TTarget, TFocusOrigin>({
  onCommands,
}: UseFindSessionOptions<TTarget, TFocusOrigin>) {
  const [state, dispatch] = useReducer(hookReducer<TTarget, TFocusOrigin>, undefined, createHookState);
  const commandHandlerRef = useRef(onCommands);
  commandHandlerRef.current = onCommands;

  useEffect(() => {
    if (state.commands.length === 0) return;
    commandHandlerRef.current(state.commands);
  }, [state.commandVersion, state.commands]);

  const send = useCallback((action: FindSessionAction<TTarget, TFocusOrigin>) => {
    dispatch(action);
  }, []);

  return { state: state.session, send } as const;
}

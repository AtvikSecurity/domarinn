// Pure reducer for the session-token slice of auth state. The token itself is
// persisted in localStorage (so the api client keeps sending it); this reducer
// is the in-memory mirror that drives React re-renders. Kept pure + tiny so it
// can be unit tested without React.

export interface AuthTokenState {
  token: string | null;
}

export type AuthTokenAction =
  | { type: "signed-in"; token: string }
  | { type: "signed-out" }
  /** Reconcile with an external change (e.g. the Settings token field). */
  | { type: "sync"; token: string | null };

export function initialAuthTokenState(token: string | null): AuthTokenState {
  return { token };
}

export function authTokenReducer(
  state: AuthTokenState,
  action: AuthTokenAction,
): AuthTokenState {
  switch (action.type) {
    case "signed-in":
      return state.token === action.token ? state : { token: action.token };
    case "signed-out":
      return state.token === null ? state : { token: null };
    case "sync":
      return state.token === action.token ? state : { token: action.token };
    default:
      return state;
  }
}

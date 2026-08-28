import { describe, expect, it } from "vitest";

import type {
  CookieSourceOption,
  CredentialStorageStatus,
  ProviderDetail,
  RegionOption,
} from "../../../types/bridge";

import {
  initialState,
  providerDetailPaneReducer,
  type ProviderDetailPaneState,
} from "./providerDetailPaneState";

const WAYFINDER_URL = "https://gateway.example.com/v1";

function baseDetail(id = "claude"): ProviderDetail {
  return {
    id,
    displayName: "Claude",
    enabled: true,
    email: "team@example.com",
    plan: "Pro",
    authType: "oauth",
    sourceLabel: "oauth",
    organization: null,
    lastUpdated: "2026-07-01T00:00:00Z",
    session: null,
    weekly: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    pace: null,
    lastError: null,
    errorState: null,
    dashboardUrl: null,
    statusPageUrl: null,
    buyCreditsUrl: null,
    hasSnapshot: true,
    cookieSource: null,
    region: null,
  };
}

const cookieOptions: CookieSourceOption[] = [
  { value: "auto", label: "Auto" },
  { value: "manual", label: "Manual" },
];

const regionOptions: RegionOption[] = [{ value: "us", label: "US" }];

const credentialStatus: CredentialStorageStatus = {
  manualCookies: "ok",
  apiKeys: "none",
  tokenAccounts: "none",
};

describe("initialState", () => {
  it("seeds gateway draft from the wayfinder url and records the synced pair", () => {
    const state = initialState(WAYFINDER_URL);
    expect(state).toEqual<ProviderDetailPaneState>({
      detail: null,
      cookieOptions: [],
      regionOptions: [],
      credentialStatus: null,
      credentialRevision: 0,
      tokenProviderIds: new Set(),
      loading: false,
      error: null,
      busy: false,
      gatewayDraft: WAYFINDER_URL,
      gatewayError: null,
      syncedProviderId: null,
      syncedGatewayUrl: WAYFINDER_URL,
    });
  });

  it("uses the provided providerId as the initial synced provider", () => {
    expect(initialState(WAYFINDER_URL, "claude").syncedProviderId).toBe("claude");
    expect(initialState(WAYFINDER_URL, null).syncedProviderId).toBeNull();
  });
});

describe("providerDetailPaneReducer — load bundle", () => {
  it("LOAD_START sets loading and clears error", () => {
    const state = initialState(WAYFINDER_URL);
    state.error = "boom";
    const next = providerDetailPaneReducer(state, { type: "LOAD_START" });
    expect(next).toEqual({ ...state, loading: true, error: null });
  });

  it("LOAD_SUCCESS stores the bundle and clears error", () => {
    const state = initialState(WAYFINDER_URL);
    state.loading = true;
    state.error = "boom";
    const detail = baseDetail();
    const next = providerDetailPaneReducer(state, {
      type: "LOAD_SUCCESS",
      detail,
      cookieOptions,
      regionOptions,
      credentialStatus,
    });
    expect(next).toEqual({
      ...state,
      error: null,
      detail,
      cookieOptions,
      regionOptions,
      credentialStatus,
    });
  });

  it("LOAD_ERROR clears the bundle and surfaces the error", () => {
    const state = initialState(WAYFINDER_URL);
    state.detail = baseDetail();
    state.cookieOptions = cookieOptions;
    state.regionOptions = regionOptions;
    state.credentialStatus = credentialStatus;
    const next = providerDetailPaneReducer(state, {
      type: "LOAD_ERROR",
      error: "nope",
    });
    expect(next).toEqual({
      ...state,
      error: "nope",
      detail: null,
      cookieOptions: [],
      regionOptions: [],
      credentialStatus: null,
    });
  });

  it("LOAD_FINISH clears loading only", () => {
    const state = initialState(WAYFINDER_URL);
    state.loading = true;
    state.detail = baseDetail();
    const next = providerDetailPaneReducer(state, { type: "LOAD_FINISH" });
    expect(next).toEqual({ ...state, loading: false });
  });
});

describe("providerDetailPaneReducer — LOAD_CLEAR modes", () => {
  it("empty mode drops stale bundle but keeps credentialStatus", () => {
    const state = initialState(WAYFINDER_URL);
    state.detail = baseDetail();
    state.cookieOptions = cookieOptions;
    state.regionOptions = regionOptions;
    state.credentialStatus = credentialStatus;
    const next = providerDetailPaneReducer(state, { type: "LOAD_CLEAR", mode: "empty" });
    expect(next).toEqual({
      ...state,
      detail: null,
      cookieOptions: [],
      regionOptions: [],
      // credentialStatus preserved
      credentialStatus,
    });
  });

  it("switch mode drops stale bundle AND credentialStatus (clears before a new load)", () => {
    const state = initialState(WAYFINDER_URL);
    state.detail = baseDetail();
    state.cookieOptions = cookieOptions;
    state.regionOptions = regionOptions;
    state.credentialStatus = credentialStatus;
    const next = providerDetailPaneReducer(state, { type: "LOAD_CLEAR", mode: "switch" });
    expect(next).toEqual({
      ...state,
      detail: null,
      cookieOptions: [],
      regionOptions: [],
      credentialStatus: null,
    });
  });
});

describe("providerDetailPaneReducer — gateway sync", () => {
  it("SYNC_PROPS is a no-op when providerId and url are unchanged", () => {
    const state = initialState(WAYFINDER_URL, "claude");
    const next = providerDetailPaneReducer(state, {
      type: "SYNC_PROPS",
      providerId: "claude",
      wayfinderGatewayUrl: WAYFINDER_URL,
    });
    expect(next).toBe(state);
  });

  it("SYNC_PROPS sets gatewayDraft to the wayfinder url for the wayfinder provider and clears gatewayError", () => {
    const state = initialState(WAYFINDER_URL, "claude");
    state.gatewayDraft = "https://old.example.com";
    state.gatewayError = "bad gateway";
    const next = providerDetailPaneReducer(state, {
      type: "SYNC_PROPS",
      providerId: "wayfinder",
      wayfinderGatewayUrl: "https://new.example.com/v1",
    });
    expect(next).toEqual({
      ...state,
      syncedProviderId: "wayfinder",
      syncedGatewayUrl: "https://new.example.com/v1",
      gatewayDraft: "https://new.example.com/v1",
      gatewayError: null,
    });
  });

  it("SYNC_PROPS keeps the existing gatewayDraft for a non-wayfinder provider", () => {
    const state = initialState(WAYFINDER_URL, "claude");
    state.gatewayDraft = "https://keep.example.com";
    state.gatewayError = "bad gateway";
    const next = providerDetailPaneReducer(state, {
      type: "SYNC_PROPS",
      providerId: "openai",
      wayfinderGatewayUrl: "https://other.example.com/v1",
    });
    expect(next).toEqual({
      ...state,
      syncedProviderId: "openai",
      syncedGatewayUrl: "https://other.example.com/v1",
      gatewayDraft: "https://keep.example.com",
      gatewayError: null,
    });
  });

  it("SYNC_PROPS with null providerId clears the synced provider", () => {
    const state = initialState(WAYFINDER_URL, "claude");
    const next = providerDetailPaneReducer(state, {
      type: "SYNC_PROPS",
      providerId: null,
      wayfinderGatewayUrl: WAYFINDER_URL,
    });
    expect(next.syncedProviderId).toBeNull();
  });
});

describe("providerDetailPaneReducer — gateway draft / save", () => {
  it("SET_GATEWAY_DRAFT replaces the draft", () => {
    const state = initialState(WAYFINDER_URL);
    const next = providerDetailPaneReducer(state, {
      type: "SET_GATEWAY_DRAFT",
      draft: "https://draft.example.com",
    });
    expect(next).toEqual({ ...state, gatewayDraft: "https://draft.example.com" });
  });

  it("SAVE_GATEWAY_START sets busy and clears gatewayError", () => {
    const state = initialState(WAYFINDER_URL);
    state.gatewayError = "old";
    const next = providerDetailPaneReducer(state, { type: "SAVE_GATEWAY_START" });
    expect(next).toEqual({ ...state, busy: true, gatewayError: null });
  });

  it("SAVE_GATEWAY_ERROR records the error", () => {
    const state = initialState(WAYFINDER_URL);
    state.busy = true;
    const next = providerDetailPaneReducer(state, {
      type: "SAVE_GATEWAY_ERROR",
      error: "rejected",
    });
    expect(next).toEqual({ ...state, gatewayError: "rejected" });
  });
});

describe("providerDetailPaneReducer — ui flags", () => {
  it("SET_BUSY toggles busy", () => {
    const state = initialState(WAYFINDER_URL);
    expect(providerDetailPaneReducer(state, { type: "SET_BUSY", busy: true }).busy).toBe(true);
    expect(
      providerDetailPaneReducer(state, { type: "SET_BUSY", busy: false }).busy,
    ).toBe(false);
  });

  it("SET_ERROR sets or clears the error", () => {
    const state = initialState(WAYFINDER_URL);
    expect(
      providerDetailPaneReducer(state, { type: "SET_ERROR", error: "oops" }).error,
    ).toBe("oops");
    expect(
      providerDetailPaneReducer(state, { type: "SET_ERROR", error: null }).error,
    ).toBeNull();
  });

  it("BUMP_CREDENTIAL_REVISION increments", () => {
    const state = initialState(WAYFINDER_URL);
    expect(
      providerDetailPaneReducer(state, { type: "BUMP_CREDENTIAL_REVISION" }).credentialRevision,
    ).toBe(1);
    expect(
      providerDetailPaneReducer(
        { ...state, credentialRevision: 5 },
        { type: "BUMP_CREDENTIAL_REVISION" },
      ).credentialRevision,
    ).toBe(6);
  });

  it("SET_TOKEN_PROVIDERS replaces the set with a new instance", () => {
    const state = initialState(WAYFINDER_URL);
    state.tokenProviderIds = new Set(["a"]);
    const next = providerDetailPaneReducer(state, {
      type: "SET_TOKEN_PROVIDERS",
      ids: ["b", "c"],
    });
    expect(next.tokenProviderIds).toBeInstanceOf(Set);
    expect([...next.tokenProviderIds]).toEqual(["b", "c"]);
    // does not mutate the prior set
    expect([...state.tokenProviderIds]).toEqual(["a"]);
  });
});

describe("providerDetailPaneReducer — unknown action", () => {
  it("returns the same state reference for an unknown action", () => {
    const state = initialState(WAYFINDER_URL);
    // @ts-expect-error exercising the default branch
    const next = providerDetailPaneReducer(state, { type: "NOPE" });
    expect(next).toBe(state);
  });
});

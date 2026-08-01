import { test as base } from "playwright-bdd";
import { customAlphabet } from "nanoid";
import { ApiHelper } from "./api.js";
import { SeedHelper } from "./seed.js";
import { spawnRdrs, spawnMockFeedServer, spawnMockKagiServer } from "./server.js";

const nano = customAlphabet("abcdefghijklmnopqrstuvwxyz0123456789", 8);

export const test = base.extend({
  kagiServer: [
    async ({}, use) => {
      const server = await spawnMockKagiServer();
      try {
        await use(server);
      } finally {
        await server.cleanup();
      }
    },
    { scope: "worker" },
  ],

  rdrsServer: [
    async ({ kagiServer }, use) => {
      const server = await spawnRdrs({ extraEnv: { RDRS_KAGI_API_BASE: kagiServer.url } });
      try {
        await use(server);
      } finally {
        await server.cleanup();
      }
    },
    { scope: "worker" },
  ],

  serverUrl: [
    async ({ rdrsServer }, use) => {
      await use(rdrsServer.url);
    },
    { scope: "worker" },
  ],

  dbPath: [
    async ({ rdrsServer }, use) => {
      await use(rdrsServer.dbPath);
    },
    { scope: "worker" },
  ],

  api: [
    async ({ serverUrl }, use) => {
      await use(new ApiHelper(serverUrl));
    },
    { scope: "worker" },
  ],

  seed: [
    async ({ dbPath }, use) => {
      await use(new SeedHelper(dbPath));
    },
    { scope: "worker" },
  ],

  feedServerUrl: [
    async ({}, use) => {
      const server = await spawnMockFeedServer();
      try {
        await use(server.url);
      } finally {
        await server.cleanup();
      }
    },
    { scope: "worker" },
  ],

  currentUser: async ({}, use) => {
    await use({ username: `e2e-${nano()}`, password: "vulture-mango-77-quilt" });
  },
});

export { expect } from "@playwright/test";

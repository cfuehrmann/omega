/**
 * WebSocket protocol — client → server messages.
 *
 * Single source of truth for the shapes of messages the Omega web client
 * sends to the server. Both sides import from here:
 *
 *   - Client (App.tsx): uses `ClientMessage` to type `sendToServer()` — compile-time enforcement.
 *   - Server (server.ts): calls `ClientMessageSchema.parse()` — runtime enforcement.
 */

import { z } from "zod";

export const OmegaModelSchema = z.enum(["claude-sonnet-4-6", "claude-opus-4-6"]);
export type OmegaModel = z.infer<typeof OmegaModelSchema>;

export const ClientMessageSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("message"), content: z.string() }),
  z.object({ type: z.literal("abort") }),
  z.object({ type: z.literal("reset") }),
  z.object({ type: z.literal("set_model"), model: OmegaModelSchema }),
]);

export type ClientMessage = z.infer<typeof ClientMessageSchema>;

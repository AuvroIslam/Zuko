import assert from "node:assert/strict";
import { test } from "node:test";
import { isPromptEndpoint, isUploadEndpoint } from "../src/shared/sites.ts";
import { rewriteJson } from "../src/main-world/rewriters.ts";
import { hasNote, stripNote } from "../src/shared/placeholders.ts";
import { AWS, CARD, KEY, MAIL, realBrain } from "./helpers.mjs";

async function setup(site) {
  const brain = await realBrain();
  const mask = async (texts) => {
    const r = await brain.maskMany(texts, { site, mode: "full" });
    return { texts: r.texts, keys: r.keys, count: r.count };
  };
  return { brain, run: (body) => rewriteJson(site, typeof body === "string" ? body : JSON.stringify(body), mask) };
}

// ---- realistic bodies --------------------------------------------------------------------

const chatgptBody = (parts, extra = {}) => ({
  action: "next",
  messages: [
    {
      id: "aaa2f3c4-71d0-4b7e-9c0e-5d3f4a8b1c22",
      author: { role: "user" },
      create_time: 1759400000.123,
      content: { content_type: "text", parts },
      metadata: { selected_github_repos: [], serialization_metadata: { custom_symbol_offsets: [] } },
    },
  ],
  conversation_id: "68f0a1b2-3c4d-4e5f-8a9b-0c1d2e3f4a5b",
  parent_message_id: "c1d2e3f4-a5b6-4c7d-8e9f-0a1b2c3d4e5f",
  model: "gpt-5",
  timezone_offset_min: -360,
  history_and_training_disabled: false,
  conversation_mode: { kind: "primary_assistant" },
  websocket_request_id: "9d8c7b6a-5e4f-4a3b-9c2d-1e0f9a8b7c6d",
  ...extra,
});

test("ChatGPT: text parts are masked, ids and metadata untouched, one note prepended", async () => {
  const { run } = await setup("chatgpt");
  const body = chatgptBody([`Here is my key ${KEY}. Put it in .env and mail ${MAIL}.`]);
  const r = await run(body);
  assert.equal(r.changed, true);
  assert.equal(r.count, 2);
  const out = JSON.parse(r.body);
  const part = out.messages[0].content.parts[0];
  assert.ok(hasNote(part));
  assert.equal(stripNote(part), "Here is my key {{API_KEY_1}}. Put it in .env and mail {{EMAIL_1}}.");
  assert.ok(!r.body.includes(KEY) && !r.body.includes(MAIL));
  // Everything except the prompt text is identical.
  const expected = structuredClone(body);
  expected.messages[0].content.parts[0] = part;
  assert.deepEqual(out, expected);
});

test("ChatGPT multimodal parts: the image pointer stays, the text part is masked", async () => {
  const { run } = await setup("chatgpt");
  const pointer = { content_type: "image_asset_pointer", asset_pointer: "file-service://file-AbC123xyz789", size_bytes: 482113, width: 1024, height: 768 };
  const body = chatgptBody([pointer, `what is wrong here? contact ${MAIL}`]);
  body.messages[0].content.content_type = "multimodal_text";
  const r = await run(body);
  const parts = JSON.parse(r.body).messages[0].content.parts;
  assert.deepEqual(parts[0], pointer);
  assert.equal(stripNote(parts[1]), "what is wrong here? contact {{EMAIL_1}}");
});

test("ChatGPT: a text part inside a content part object is masked too", async () => {
  const { run } = await setup("chatgpt");
  const body = chatgptBody([{ content_type: "audio_transcription", text: `my card is ${CARD}`, direction: "in", decoding_id: null }]);
  const r = await run(body);
  const part = JSON.parse(r.body).messages[0].content.parts[0];
  assert.ok(part.text.includes("{{CARD_1}}") && !part.text.includes("4242"));
  assert.equal(part.direction, "in");
});

test("a body with nothing sensitive is returned byte for byte", async () => {
  const { run } = await setup("chatgpt");
  const raw = JSON.stringify(chatgptBody(["How do I reverse a list in Python?"]));
  const r = await run(raw);
  assert.equal(r.changed, false);
  assert.equal(r.body, raw);
  assert.equal(r.count, 0);
});

test("the /prepare call and regenerate (no messages) are not touched", async () => {
  const { run } = await setup("chatgpt");
  const prepare = JSON.stringify({ action: "next", fork_from_shared_post: false, parent_message_id: "x", model: "gpt-5", timezone_offset_min: -360 });
  assert.equal(await run(prepare), null);
  assert.equal(await run("not json"), null);
});

test("rewriting is idempotent: re-sending an edited message does not stack notes or re-mask", async () => {
  const { run } = await setup("chatgpt");
  const first = await run(chatgptBody([`token ${KEY}`]));
  const second = await run(first.body);
  assert.equal(second.body, first.body);
  const part = JSON.parse(second.body).messages[0].content.parts[0];
  assert.equal(part.split("[Zuko privacy note:").length, 2);
});

test("claude.ai: prompt and attachment text are masked; the note goes on the prompt", async () => {
  const { run } = await setup("claude");
  const body = {
    prompt: `Summarize and keep ${AWS} out of it. I am ${MAIL}`,
    timezone: "Asia/Dhaka",
    personalized_styles: [{ type: "default", key: "Default", name: "Normal", nameKey: "normal_style_name", prompt: "Normal\n", summary: "Default responses from Claude", isDefault: true }],
    locale: "en-US",
    tools: [],
    attachments: [{ file_name: "notes.txt", file_size: 183, file_type: "txt", extracted_content: `DB_PASSWORD=x\nAWS key: ${AWS}\nowner ${MAIL}\n` }],
    files: [],
    sync_sources: [],
    rendering_mode: "messages",
  };
  const r = await run(body);
  const out = JSON.parse(r.body);
  assert.ok(hasNote(out.prompt));
  assert.equal(stripNote(out.prompt), "Summarize and keep {{API_KEY_1}} out of it. I am {{EMAIL_1}}");
  assert.equal(out.attachments[0].extracted_content, "DB_PASSWORD=x\nAWS key: {{API_KEY_1}}\nowner {{EMAIL_1}}\n");
  assert.equal(out.attachments[0].file_name, "notes.txt");
  assert.deepEqual(out.personalized_styles, body.personalized_styles);
  assert.ok(!r.body.includes(AWS));
});

test("claude.ai: a sanitized attachment alone still gets the note on an empty prompt", async () => {
  const { run } = await setup("claude");
  const r = await run({ prompt: "", attachments: [{ file_name: "a.txt", extracted_content: `k ${KEY}` }], files: [] });
  const out = JSON.parse(r.body);
  assert.ok(hasNote(out.prompt));
  assert.ok(out.attachments[0].extracted_content.includes("{{API_KEY_1}}"));
});

test("DeepSeek completion and edit_message: prompt masked, session fields untouched", async () => {
  const { run } = await setup("deepseek");
  const completion = { chat_session_id: "0b7c2d9e-1a2b-4c3d-9e8f-7a6b5c4d3e2f", parent_message_id: 4, prompt: `my key is ${KEY}`, ref_file_ids: ["file-1234"], thinking_enabled: true, search_enabled: false };
  const r = await run(completion);
  const out = JSON.parse(r.body);
  assert.equal(stripNote(out.prompt), "my key is {{API_KEY_1}}");
  assert.deepEqual({ ...out, prompt: "" }, { ...completion, prompt: "" });
  const edit = { chat_session_id: completion.chat_session_id, message_id: 7, prompt: `edited: ${MAIL}`, search_enabled: false, thinking_enabled: false };
  const e = JSON.parse((await run(edit)).body);
  assert.equal(stripNote(e.prompt), "edited: {{EMAIL_1}}");
  assert.equal(e.message_id, 7);
});

test("JSON-significant characters in a masked value survive re-serialization", async () => {
  const { brain, run } = await setup("deepseek");
  await brain.applyDetector({ customTerms: ['Project "Falcon"\\Q3'] });
  const prompt = 'status of Project "Falcon"\\Q3 -\né  ok';
  const r = await run({ prompt });
  const out = JSON.parse(r.body);
  assert.equal(stripNote(out.prompt), "status of {{TERM_1}} -\né  ok");
});

test("endpoint matching: known prompt and upload endpoints per site, nothing else", () => {
  const u = (s) => new URL(s);
  assert.ok(isPromptEndpoint("chatgpt", u("https://chatgpt.com/backend-api/f/conversation"), "POST"));
  assert.ok(isPromptEndpoint("chatgpt", u("https://chatgpt.com/backend-api/conversation"), "POST"));
  assert.ok(isPromptEndpoint("chatgpt", u("https://chatgpt.com/backend-anon/f/conversation"), "POST"));
  assert.ok(isPromptEndpoint("chatgpt", u("https://chatgpt.com/backend-api/f/conversation/prepare"), "POST"));
  assert.ok(!isPromptEndpoint("chatgpt", u("https://chatgpt.com/backend-api/conversations?offset=0"), "GET"));
  assert.ok(!isPromptEndpoint("chatgpt", u("https://chatgpt.com/backend-api/f/conversation"), "GET"));
  assert.ok(isPromptEndpoint("claude", u("https://claude.ai/api/organizations/1f2e/chat_conversations/9a8b-7c/completion"), "POST"));
  assert.ok(isPromptEndpoint("claude", u("https://claude.ai/api/organizations/1f2e/chat_conversations/9a8b-7c/retry_completion"), "POST"));
  assert.ok(!isPromptEndpoint("claude", u("https://claude.ai/api/organizations/1f2e/chat_conversations"), "POST"));
  assert.ok(isPromptEndpoint("deepseek", u("https://chat.deepseek.com/api/v0/chat/completion"), "POST"));
  assert.ok(isPromptEndpoint("deepseek", u("https://chat.deepseek.com/api/v0/chat/edit_message"), "POST"));
  assert.ok(isUploadEndpoint("deepseek", u("https://chat.deepseek.com/api/v0/file/upload_file"), "POST"));
  assert.ok(isUploadEndpoint("chatgpt", u("https://chatgpt.com/backend-api/files"), "POST"));
  assert.ok(!isUploadEndpoint("chatgpt", u("https://evil-oaiusercontent.com/files/abc?sig=1"), "PUT"), "host must match the storage domain exactly");
  assert.ok(isUploadEndpoint("chatgpt", u("https://files.oaiusercontent.com/file-abc?se=1"), "PUT"));
  assert.ok(isUploadEndpoint("claude", u("https://claude.ai/api/organizations/1f2e/upload"), "POST"));
});

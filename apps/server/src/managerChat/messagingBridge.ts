import { createHash, randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import {
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  writeFileSync
} from 'node:fs';
import { basename, dirname, join } from 'node:path';
import type { PermissionPublish } from './ManagerChatManager.js';
import { respondManagerChatPermission, sendManagerChatMessage } from './ManagerChatManager.js';
import { stateBase } from './chatSessions.js';

const MAX_TEXT = 4_000;
const MAX_ATTACHMENT_BYTES = 64 * 1024;
const ACTION_TTL_MS = 5 * 60_000;
const TEXT_MIME_TYPES = new Set(['text/plain', 'text/markdown', 'application/json']);
const UNSAFE_CONTROL = /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/;

type BridgeRole = 'owner' | 'chat';

export interface BridgeOperator {
  id: string;
  gateway: 'telegram';
  externalUserId: string;
  chatId: string;
  role: BridgeRole;
  profiles: string[];
  pairedAt: string;
  revokedAt: string | null;
}

interface TelegramDocument {
  file_id?: unknown;
  file_name?: unknown;
  file_size?: unknown;
  mime_type?: unknown;
}

interface TelegramUpdate {
  update_id?: unknown;
  message?: {
    text?: unknown;
    caption?: unknown;
    from?: { id?: unknown };
    chat?: { id?: unknown };
    document?: TelegramDocument;
    photo?: unknown;
    audio?: unknown;
    video?: unknown;
    voice?: unknown;
    sticker?: unknown;
  };
  callback_query?: {
    id?: unknown;
    data?: unknown;
    from?: { id?: unknown };
    message?: { chat?: { id?: unknown } };
  };
}

interface Receipt {
  eventId: string;
  requestId: string;
  operatorId: string;
  messageDigest: string;
  kind: 'message' | 'callback';
  chatId: string;
  callbackId?: string;
  status: 'processing' | 'reply_ready' | 'delivered' | 'rejected';
  reply?: string;
  profile?: string;
  updatedAt: string;
}

interface StoredAction {
  operatorId: string;
  profile: string;
  sessionId?: string;
  permissionId: string;
  optionId: string;
  optionKind: 'allow_once' | 'reject_once';
  expiresAt: number;
}

export interface TelegramButton {
  text: string;
  data: string;
}

export interface TelegramTransport {
  send(chatId: string, text: string, buttons?: TelegramButton[]): Promise<void>;
  answerCallback(callbackId: string, text: string): Promise<void>;
  loadTextDocument(document: Required<Pick<TelegramDocument, 'file_id' | 'file_name' | 'file_size' | 'mime_type'>>): Promise<string>;
}

type SendTurn = (input: {
  profile: string;
  text: string;
  requestId: string;
  onPermission: PermissionPublish;
}) => Promise<string>;

export interface MessagingBridgeOptions {
  stateDir?: string;
  telegramSecret?: string;
  transport?: TelegramTransport;
  sendTurn?: SendTurn;
  respondPermission?: typeof respondManagerChatPermission;
}

function digest(value: string): string {
  return createHash('sha256').update(value).digest('hex');
}

function bounded(value: unknown, max: number): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= max && !UNSAFE_CONTROL.test(value);
}

export function redactBridgeText(value: string): string {
  return value
    .replace(/\b(?:gh[pousr]_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9_-]{20,})\b/g, '[REDACTED:TOKEN]')
    .replace(/(authorization\s*:\s*bearer\s+)[^\s]+/gi, '$1[REDACTED:TOKEN]')
    .replace(/((?:api[_-]?key|token|secret|password)\s*[=:]\s*)[^\s]+/gi, '$1[REDACTED:SECRET]');
}

function telegramId(value: unknown, allowNegative = false): string | null {
  const text = typeof value === 'number' && Number.isSafeInteger(value) ? String(value) : typeof value === 'string' ? value : '';
  return new RegExp(allowNegative ? '^-?\\d{1,20}$' : '^\\d{1,20}$').test(text) ? text : null;
}

function safeFilename(value: string): string {
  return redactBridgeText(basename(value).replace(/[\x00-\x1f\x7f]/g, '')).slice(0, 100) || 'attachment.txt';
}

class FetchTelegramTransport implements TelegramTransport {
  constructor(private token = process.env.TELEGRAM_BOT_TOKEN?.trim() ?? '') {}

  private async api(method: string, body: object): Promise<unknown> {
    if (!this.token) throw new Error('TELEGRAM_BOT_TOKEN is not configured');
    let response: Response;
    try {
      response = await fetch(`https://api.telegram.org/bot${this.token}/${method}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(10_000)
      });
    } catch { throw new Error(`Telegram ${method} transport failed`); }
    const payload = await response.json().catch(() => null) as { ok?: boolean; result?: unknown; description?: string } | null;
    if (!response.ok || payload?.ok !== true) throw new Error(`Telegram ${method} failed with HTTP ${response.status}: ${payload?.description ?? 'unknown error'}`);
    return payload.result;
  }

  async send(chatId: string, text: string, buttons: TelegramButton[] = []): Promise<void> {
    await this.api('sendMessage', {
      chat_id: chatId,
      text,
      ...(buttons.length > 0 ? { reply_markup: { inline_keyboard: buttons.map((button) => [{ text: button.text, callback_data: button.data }]) } } : {})
    });
  }

  async answerCallback(callbackId: string, text: string): Promise<void> {
    await this.api('answerCallbackQuery', { callback_query_id: callbackId, text });
  }

  async loadTextDocument(document: Required<Pick<TelegramDocument, 'file_id' | 'file_name' | 'file_size' | 'mime_type'>>): Promise<string> {
    const result = await this.api('getFile', { file_id: document.file_id }) as { file_path?: unknown };
    if (!bounded(result?.file_path, 512)) throw new Error('Telegram did not return a valid attachment path');
    let response: Response;
    try { response = await fetch(`https://api.telegram.org/file/bot${this.token}/${result.file_path}`, { signal: AbortSignal.timeout(10_000) }); }
    catch { throw new Error('Telegram attachment download failed'); }
    if (!response.ok) throw new Error(`Telegram attachment download failed with HTTP ${response.status}`);
    const length = Number(response.headers.get('content-length'));
    if (Number.isFinite(length) && length > MAX_ATTACHMENT_BYTES) throw new Error('Attachment exceeds 64 KiB');
    if (!response.body) throw new Error('Telegram attachment had no body');
    const reader = response.body.getReader();
    const chunks: Uint8Array[] = [];
    let total = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > MAX_ATTACHMENT_BYTES) { await reader.cancel(); throw new Error('Attachment exceeds 64 KiB'); }
      chunks.push(value);
    }
    const bytes = Buffer.concat(chunks, total);
    try { return new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
    catch { throw new Error('Attachment is not valid UTF-8 text'); }
  }
}

export class MessagingBridge {
  private directory: string;
  private transport: TelegramTransport;
  private sendTurn: SendTurn;
  private respondPermission: typeof respondManagerChatPermission;
  private telegramSecret: string;

  constructor(options: MessagingBridgeOptions = {}) {
    this.directory = options.stateDir ?? join(stateBase(), 'messaging-bridge');
    this.transport = options.transport ?? new FetchTelegramTransport();
    this.telegramSecret = options.telegramSecret ?? process.env.GAH_TELEGRAM_WEBHOOK_SECRET?.trim() ?? '';
    if (this.telegramSecret && !/^[A-Za-z0-9_-]{1,256}$/.test(this.telegramSecret)) {
      throw new Error('GAH_TELEGRAM_WEBHOOK_SECRET must contain 1–256 letters, digits, underscores, or hyphens.');
    }
    this.respondPermission = options.respondPermission ?? respondManagerChatPermission;
    this.sendTurn = options.sendTurn ?? (async ({ profile, text, requestId, onPermission }) =>
      (await sendManagerChatMessage(profile, text, requestId, undefined, undefined, undefined, { onPermission })).turn.text);
  }

  telegramAuthenticated(value: string | undefined): boolean {
    if (!this.telegramSecret || !value) return false;
    return timingSafeEqual(Buffer.from(digest(value)), Buffer.from(digest(this.telegramSecret)));
  }

  listOperators(): BridgeOperator[] {
    return this.readOperators();
  }

  pair(input: { externalUserId?: unknown; chatId?: unknown; role?: unknown; profiles?: unknown }): BridgeOperator {
    const externalUserId = telegramId(input.externalUserId);
    const chatId = telegramId(input.chatId, true);
    const role = input.role;
    const profiles = Array.isArray(input.profiles) ? [...new Set(input.profiles)] : [];
    if (!externalUserId || !chatId || (role !== 'owner' && role !== 'chat') || profiles.length === 0 || profiles.length > 32
      || profiles.some((profile) => !bounded(profile, 128) || profile.trim() !== profile)) {
      throw new Error('A Telegram user/chat, owner|chat role, and 1–32 exact profile names are required.');
    }
    const operators = this.readOperators();
    const now = new Date().toISOString();
    const existing = operators.find((operator) => operator.gateway === 'telegram'
      && operator.externalUserId === externalUserId && operator.chatId === chatId);
    const paired: BridgeOperator = existing
      ? { ...existing, role, profiles: profiles as string[], pairedAt: now, revokedAt: null }
      : { id: randomUUID(), gateway: 'telegram', externalUserId, chatId, role, profiles: profiles as string[], pairedAt: now, revokedAt: null };
    if (existing) operators[operators.indexOf(existing)] = paired;
    else operators.push(paired);
    this.writeJson('operators.json', operators);
    this.audit('operator.paired', paired.id, null, { role, profiles: paired.profiles });
    return paired;
  }

  revoke(operatorId: unknown): BridgeOperator {
    if (!bounded(operatorId, 128)) throw new Error('operatorId is required.');
    const operators = this.readOperators();
    const index = operators.findIndex((operator) => operator.id === operatorId && operator.revokedAt === null);
    if (index < 0) throw new Error('Active bridge operator not found.');
    operators[index] = { ...operators[index], revokedAt: new Date().toISOString() };
    this.writeJson('operators.json', operators);
    this.audit('operator.revoked', operators[index].id, null);
    return operators[index];
  }

  async handleTelegram(update: unknown): Promise<{ duplicate: boolean; eventId: string }> {
    const parsed = this.parseUpdate(update);
    const eventId = `telegram:${parsed.updateId}`;
    const operator = this.authorize(parsed.userId, parsed.chatId);
    if (!operator) {
      this.audit('update.unauthorized', 'unpaired', eventId);
      return { duplicate: false, eventId };
    }
    const requestId = `bridge-${digest(eventId).slice(0, 24)}`;
    const claimed = this.claimReceipt({
      eventId,
      requestId,
      operatorId: operator.id,
      messageDigest: digest(JSON.stringify(update)),
      kind: parsed.kind,
      chatId: parsed.chatId,
      ...(parsed.callbackId ? { callbackId: parsed.callbackId } : {}),
      status: 'processing',
      updatedAt: new Date().toISOString()
    });
    if (!claimed.created) {
      if (claimed.receipt.messageDigest !== digest(JSON.stringify(update))) throw new Error('Update id was reused with different content.');
      if (claimed.receipt.status === 'reply_ready') await this.deliverReceipt(claimed.receipt);
      return { duplicate: true, eventId };
    }

    try {
      if (parsed.kind === 'callback') {
        const reply = await this.handleAction(operator, parsed.actionToken!);
        await this.finishReceipt(claimed.receipt, reply);
        return { duplicate: false, eventId };
      }
      const { profile, prompt } = await this.prompt(operator, parsed);
      claimed.receipt.profile = profile;
      this.saveReceipt(claimed.receipt);
      const reply = redactBridgeText(await this.sendTurn({
        profile,
        text: `[MessagingEvent ${eventId}; authenticated role=${operator.role}. Free text is not approval. Require a typed one-time permission before any mutation.]\n${prompt}`,
        requestId,
        onPermission: (event) => this.publishPermission(operator, event)
      })).slice(0, MAX_TEXT);
      await this.finishReceipt(claimed.receipt, reply || 'Manager turn completed without a text reply.');
      return { duplicate: false, eventId };
    } catch (error) {
      if (claimed.receipt.status === 'reply_ready') throw error;
      const reply = redactBridgeText(error instanceof Error ? error.message : String(error)).slice(0, MAX_TEXT);
      claimed.receipt.status = 'rejected';
      claimed.receipt.reply = reply;
      claimed.receipt.updatedAt = new Date().toISOString();
      this.saveReceipt(claimed.receipt);
      this.audit('update.rejected', operator.id, eventId, { reason: digest(reply) });
      await this.finishReceipt(claimed.receipt, reply);
      return { duplicate: false, eventId };
    }
  }

  private parseUpdate(value: unknown): {
    updateId: string;
    kind: 'message' | 'callback';
    userId: string;
    chatId: string;
    text?: string;
    document?: TelegramDocument;
    unsupportedAttachment?: boolean;
    callbackId?: string;
    actionToken?: string;
  } {
    if (!value || typeof value !== 'object') throw new Error('Invalid Telegram update.');
    const update = value as TelegramUpdate;
    const updateId = telegramId(update.update_id);
    if (!updateId) throw new Error('Invalid Telegram update id.');
    if (update.callback_query) {
      const userId = telegramId(update.callback_query.from?.id);
      const chatId = telegramId(update.callback_query.message?.chat?.id, true);
      const callbackId = bounded(update.callback_query.id, 128) ? update.callback_query.id : null;
      const data = bounded(update.callback_query.data, 128) ? update.callback_query.data : '';
      if (!userId || !chatId || !callbackId || !data.startsWith('gah:')) throw new Error('Invalid Telegram action callback.');
      return { updateId, kind: 'callback', userId, chatId, callbackId, actionToken: data.slice(4) };
    }
    const message = update.message;
    const userId = telegramId(message?.from?.id);
    const chatId = telegramId(message?.chat?.id, true);
    if (!message || !userId || !chatId) throw new Error('Invalid Telegram message.');
    const text = typeof message.text === 'string' ? message.text : typeof message.caption === 'string' ? message.caption : '';
    return {
      updateId,
      kind: 'message',
      userId,
      chatId,
      text,
      document: message.document,
      unsupportedAttachment: !!(message.photo || message.audio || message.video || message.voice || message.sticker)
    };
  }

  private authorize(userId: string, chatId: string): BridgeOperator | null {
    return this.readOperators().find((operator) => operator.revokedAt === null
      && operator.externalUserId === userId && operator.chatId === chatId) ?? null;
  }

  private async prompt(operator: BridgeOperator, input: ReturnType<MessagingBridge['parseUpdate']>): Promise<{ profile: string; prompt: string }> {
    if (input.unsupportedAttachment) throw new Error('Only one UTF-8 text document up to 64 KiB is accepted.');
    let text = input.text?.trim() ?? '';
    let profile: string;
    if (text.startsWith('/chat ')) {
      const match = /^\/chat\s+(\S+)\s+([\s\S]+)$/.exec(text);
      if (!match || !operator.profiles.includes(match[1])) throw new Error('Use /chat <paired-profile> <message>.');
      profile = match[1];
      text = match[2];
    } else {
      if (text.startsWith('/')) throw new Error('Only /chat <profile> <message> is accepted; remote slash commands are disabled.');
      if (operator.profiles.length !== 1) throw new Error('Use /chat <paired-profile> <message>.');
      profile = operator.profiles[0];
    }
    if (!text && input.document) text = 'Review the attached document.';
    if (!bounded(text, MAX_TEXT)) throw new Error('Message text must contain 1–4,000 safe characters.');
    let prompt = redactBridgeText(text);
    if (input.document) {
      const document = input.document;
      if (!bounded(document.file_id, 256) || !bounded(document.file_name, 255)
        || typeof document.file_size !== 'number' || !Number.isSafeInteger(document.file_size)
        || document.file_size < 0 || document.file_size > MAX_ATTACHMENT_BYTES
        || typeof document.mime_type !== 'string' || !TEXT_MIME_TYPES.has(document.mime_type)) {
        throw new Error('Only one UTF-8 text document up to 64 KiB is accepted.');
      }
      const attachment = await this.transport.loadTextDocument(document as Required<Pick<TelegramDocument, 'file_id' | 'file_name' | 'file_size' | 'mime_type'>>);
      if (Buffer.byteLength(attachment, 'utf8') > MAX_ATTACHMENT_BYTES || UNSAFE_CONTROL.test(attachment)) throw new Error('Attachment is not safe bounded text.');
      prompt = `${prompt}\n\nAttachment ${safeFilename(document.file_name as string)}:\n${redactBridgeText(attachment)}`;
    }
    return { profile, prompt };
  }

  private async publishPermission(operator: BridgeOperator, event: Parameters<PermissionPublish>[0]): Promise<void> {
    const reject = event.options.find((option) => option.kind === 'reject_once');
    if (operator.role !== 'owner') {
      await this.respondPermission(event.profile, event.sessionId, event.permissionId, reject?.optionId ?? 'cancelled');
      await this.transport.send(operator.chatId, 'A manager action was denied. Remote approvals require an owner-paired identity.');
      return;
    }
    const safeOptions = event.options.filter((option): option is typeof option & { kind: 'allow_once' | 'reject_once' } =>
      (option.kind === 'allow_once' || option.kind === 'reject_once')
      && bounded(option.optionId, 256) && bounded(option.name, 100));
    if (safeOptions.length === 0) throw new Error('This action has no bounded one-time approval option. Use the dashboard.');
    const buttons: TelegramButton[] = safeOptions.map((option) => {
      const token = randomBytes(18).toString('base64url');
      this.writeJson(join('actions', `${token}.json`), {
        operatorId: operator.id,
        profile: event.profile,
        ...(event.sessionId ? { sessionId: event.sessionId } : {}),
        permissionId: event.permissionId,
        optionId: option.optionId,
        optionKind: option.kind,
        expiresAt: Date.now() + ACTION_TTL_MS
      } satisfies StoredAction, true);
      return { text: redactBridgeText(option.name).slice(0, 40), data: `gah:${token}` };
    });
    const locations = event.locations.slice(0, 3).map((location) => safeFilename(location)).join(', ');
    const title = redactBridgeText(event.title).slice(0, 500);
    await this.transport.send(operator.chatId, `Confirm one exact manager action:\n${title}${locations ? `\nLocations: ${locations}` : ''}`, buttons);
    this.audit('action.offered', operator.id, `manager:${event.requestId}`, { profile: event.profile, permissionId: digest(event.permissionId) });
  }

  private async handleAction(operator: BridgeOperator, token: string): Promise<string> {
    if (!/^[A-Za-z0-9_-]{24}$/.test(token) || operator.role !== 'owner') throw new Error('Action is invalid or requires an owner-paired identity.');
    const path = join(this.directory, 'actions', `${token}.json`);
    if (!existsSync(path)) throw new Error('Action expired or was already used.');
    const action = JSON.parse(readFileSync(path, 'utf8')) as StoredAction;
    if (action.operatorId !== operator.id || !operator.profiles.includes(action.profile) || action.expiresAt < Date.now()) {
      throw new Error('Action expired or is outside this operator scope.');
    }
    renameSync(path, `${path}.used`);
    const answered = await this.respondPermission(action.profile, action.sessionId, action.permissionId, action.optionId);
    if (!answered) throw new Error('The manager action is no longer pending.');
    this.audit('action.answered', operator.id, null, { profile: action.profile, permissionId: digest(action.permissionId), optionKind: action.optionKind });
    return 'Manager action recorded.';
  }

  private async finishReceipt(receipt: Receipt, reply: string): Promise<void> {
    receipt.reply = reply;
    receipt.status = 'reply_ready';
    receipt.updatedAt = new Date().toISOString();
    this.saveReceipt(receipt);
    await this.deliverReceipt(receipt);
  }

  private async deliverReceipt(receipt: Receipt): Promise<void> {
    if (!receipt.reply) throw new Error('Bridge receipt has no reply to deliver.');
    if (receipt.kind === 'callback') await this.transport.answerCallback(receipt.callbackId!, receipt.reply);
    else await this.transport.send(receipt.chatId, receipt.reply);
    receipt.status = 'delivered';
    receipt.updatedAt = new Date().toISOString();
    this.saveReceipt(receipt);
    this.audit('update.delivered', receipt.operatorId, receipt.eventId, { profile: receipt.profile ?? null });
  }

  private readOperators(): BridgeOperator[] {
    const path = join(this.directory, 'operators.json');
    if (!existsSync(path)) return [];
    const parsed = JSON.parse(readFileSync(path, 'utf8'));
    if (!Array.isArray(parsed)) throw new Error('Invalid messaging bridge operator store.');
    return parsed as BridgeOperator[];
  }

  private claimReceipt(receipt: Receipt): { created: boolean; receipt: Receipt } {
    const path = this.receiptPath(receipt.eventId);
    mkdirSync(join(this.directory, 'receipts'), { recursive: true, mode: 0o700 });
    let file: number;
    try { file = openSync(path, 'wx', 0o600); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
      return { created: false, receipt: JSON.parse(readFileSync(path, 'utf8')) as Receipt };
    }
    try { writeFileSync(file, JSON.stringify(receipt)); fsyncSync(file); }
    finally { closeSync(file); }
    this.syncDirectory(dirname(path));
    this.audit('update.accepted', receipt.operatorId, receipt.eventId);
    return { created: true, receipt };
  }

  private receiptPath(eventId: string): string {
    return join(this.directory, 'receipts', `${digest(eventId)}.json`);
  }

  private saveReceipt(receipt: Receipt): void {
    this.writeJson(join('receipts', `${digest(receipt.eventId)}.json`), receipt);
  }

  private writeJson(relative: string, value: unknown, exclusive = false): void {
    const path = join(this.directory, relative);
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    if (exclusive) {
      const file = openSync(path, 'wx', 0o600);
      try { writeFileSync(file, JSON.stringify(value)); fsyncSync(file); }
      finally { closeSync(file); }
      this.syncDirectory(dirname(path));
      return;
    }
    const temporary = `${path}.${process.pid}.${randomBytes(4).toString('hex')}.tmp`;
    const file = openSync(temporary, 'wx', 0o600);
    try { writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`); fsyncSync(file); }
    finally { closeSync(file); }
    renameSync(temporary, path);
    this.syncDirectory(dirname(path));
  }

  private audit(action: string, operatorId: string, eventId: string | null, details: object = {}): void {
    mkdirSync(this.directory, { recursive: true, mode: 0o700 });
    const path = join(this.directory, 'audit.jsonl');
    const file = openSync(path, 'a', 0o600);
    try {
      writeFileSync(file, `${JSON.stringify({ timestamp: new Date().toISOString(), action, operatorId, eventId, ...details })}\n`);
      fsyncSync(file);
    } finally { closeSync(file); }
  }

  private syncDirectory(directory: string): void {
    const folder = openSync(directory, 'r');
    try { fsyncSync(folder); } finally { closeSync(folder); }
  }
}

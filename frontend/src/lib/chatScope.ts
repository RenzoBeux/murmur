/**
 * What a chat conversation is *about*.
 *
 * The meeting chat and the project chat are the same conversation UI over two
 * different bodies of source material. Everything that differs between them —
 * the command names and the id argument — is collected here, so the hooks and
 * the panel stay scope-agnostic and there is exactly one place to look when a
 * third scope appears.
 */
export type ChatScope =
  | { kind: 'meeting'; meetingId: string }
  | { kind: 'project'; projectId: string };

/**
 * A stable string identifying the scope.
 *
 * Used as the hooks' effect dependency and stale-response guard rather than the
 * scope object itself, which callers rebuild on every render.
 */
export function chatScopeKey(scope: ChatScope): string {
  return scope.kind === 'meeting' ? `meeting/${scope.meetingId}` : `project/${scope.projectId}`;
}

/** The id, or '' when the page has not resolved one yet. Hooks bail on ''. */
export function chatScopeId(scope: ChatScope): string {
  return scope.kind === 'meeting' ? scope.meetingId : scope.projectId;
}

/** The scope half of every chat `invoke` argument object. */
export function chatScopeArgs(scope: ChatScope): Record<string, string> {
  return scope.kind === 'meeting'
    ? { meetingId: scope.meetingId }
    : { projectId: scope.projectId };
}

interface ChatCommandSet {
  getHistory: string;
  sendMessage: string;
  clearHistory: string;
  listThreads: string;
  createThread: string;
  deleteThread: string;
  setThreadGrounding: string;
}

const MEETING_COMMANDS: ChatCommandSet = {
  getHistory: 'api_get_chat_history',
  sendMessage: 'api_send_chat_message',
  clearHistory: 'api_clear_chat_history',
  listThreads: 'api_list_chat_threads',
  createThread: 'api_create_chat_thread',
  deleteThread: 'api_delete_chat_thread',
  setThreadGrounding: 'api_set_chat_thread_grounding',
};

const PROJECT_COMMANDS: ChatCommandSet = {
  getHistory: 'api_get_project_chat_history',
  sendMessage: 'api_send_project_chat_message',
  clearHistory: 'api_clear_project_chat_history',
  listThreads: 'api_list_project_chat_threads',
  createThread: 'api_create_project_chat_thread',
  deleteThread: 'api_delete_project_chat_thread',
  setThreadGrounding: 'api_set_project_chat_thread_grounding',
};

export function chatCommands(scope: ChatScope): ChatCommandSet {
  return scope.kind === 'meeting' ? MEETING_COMMANDS : PROJECT_COMMANDS;
}

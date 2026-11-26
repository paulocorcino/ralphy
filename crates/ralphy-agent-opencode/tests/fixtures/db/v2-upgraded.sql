BEGIN TRANSACTION;
CREATE TABLE `account` (
          `id` text PRIMARY KEY,
          `email` text NOT NULL,
          `url` text NOT NULL,
          `access_token` text NOT NULL,
          `refresh_token` text NOT NULL,
          `token_expiry` integer,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `account_state` (
          `id` integer PRIMARY KEY,
          `active_account_id` text,
          `active_org_id` text,
          CONSTRAINT `fk_account_state_active_account_id_account_id_fk` FOREIGN KEY (`active_account_id`) REFERENCES `account`(`id`) ON DELETE SET NULL
        );
CREATE TABLE `automation` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `session_id` text,
          `directory` text NOT NULL,
          `name` text NOT NULL,
          `trigger` text NOT NULL,
          `tz` text NOT NULL,
          `action` text NOT NULL,
          `status` text DEFAULT 'active' NOT NULL,
          `next_due_at` integer,
          `expires_at` integer,
          `run_count` integer DEFAULT 0 NOT NULL,
          `failures` integer DEFAULT 0 NOT NULL,
          `last_run_at` integer,
          `last_message_id` text,
          `last_outcome` text,
          `last_output` text,
          `retry_at` integer,
          `state` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        , `event_position` text);
CREATE TABLE `blob` (
          `hash` text PRIMARY KEY,
          `mime` text NOT NULL,
          `bytes` integer NOT NULL,
          `width` integer,
          `height` integer,
          `data` blob NOT NULL,
          `time_created` integer NOT NULL
        );
CREATE TABLE `blob_ref` (
          `session_id` text NOT NULL,
          `message_id` text NOT NULL,
          `hash` text NOT NULL,
          `seq` integer NOT NULL,
          CONSTRAINT `blob_ref_pk` PRIMARY KEY(`session_id`, `message_id`, `hash`),
          CONSTRAINT `fk_blob_ref_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE,
          CONSTRAINT `fk_blob_ref_hash_blob_hash_fk` FOREIGN KEY (`hash`) REFERENCES `blob`(`hash`) ON DELETE CASCADE
        );
CREATE TABLE `control_account` (
          `email` text NOT NULL,
          `url` text NOT NULL,
          `access_token` text NOT NULL,
          `refresh_token` text NOT NULL,
          `token_expiry` integer,
          `active` integer NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `control_account_pk` PRIMARY KEY(`email`, `url`)
        );
CREATE TABLE `credential` (
          `id` text PRIMARY KEY,
          `integration_id` text,
          `label` text NOT NULL,
          `value` text NOT NULL,
          `connector_id` text,
          `method_id` text,
          `active` integer,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `event` (
          `id` text PRIMARY KEY,
          `aggregate_id` text NOT NULL,
          `seq` integer NOT NULL,
          `type` text NOT NULL,
          `data` text NOT NULL, `created` integer DEFAULT 0 NOT NULL,
          CONSTRAINT `fk_event_aggregate_id_event_sequence_aggregate_id_fk` FOREIGN KEY (`aggregate_id`) REFERENCES `event_sequence`(`aggregate_id`) ON DELETE CASCADE
        );
CREATE TABLE `event_sequence` (
          `aggregate_id` text PRIMARY KEY,
          `seq` integer NOT NULL,
          `owner_id` text
        );
CREATE TABLE `instruction_blob` (
          `hash` text PRIMARY KEY,
          `value` text
        );
CREATE TABLE `instruction_entry` (
          `session_id` text NOT NULL,
          `key` text NOT NULL,
          `value` text,
          `removed` integer DEFAULT false NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `instruction_entry_pk` PRIMARY KEY(`session_id`, `key`),
          CONSTRAINT `fk_instruction_entry_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `instruction_state` (
          `session_id` text PRIMARY KEY,
          `epoch_start` integer NOT NULL,
          `through_seq` integer NOT NULL,
          `initial_values` text NOT NULL,
          `current_values` text NOT NULL,
          CONSTRAINT `fk_instruction_state_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `job` (
          `notification_id` text PRIMARY KEY,
          `id` text NOT NULL,
          `session_id` text NOT NULL,
          `child_session_id` text,
          `kind` text NOT NULL,
          `title` text,
          `status` text NOT NULL,
          `output` text,
          `error` text,
          `recovery` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `kv` (
          `key` text PRIMARY KEY,
          `value` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `message` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_message_session_id_session_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session`(`id`) ON DELETE CASCADE
        );
INSERT INTO "message" VALUES('msg_212476c8e001NBTO6NuYEtXANQ','ses_dedb89c6affe2Bf0J714lbuYfg',1795603000462,1795603016720,'{"role":"user","time":{"created":1795603000462},"agent":"build","model":{"providerID":"opencode","modelID":"big-pickle"},"summary":{"diffs":[]}}');
INSERT INTO "message" VALUES('msg_2124778a3001mTp67wZXE5NuVr','ses_dedb89c6affe2Bf0J714lbuYfg',1795603003555,1795603032046,'{"parentID":"msg_212476c8e001NBTO6NuYEtXANQ","role":"assistant","mode":"build","agent":"build","path":{"cwd":"<scratch>\\repos\\fx-v1","root":"<scratch>\\repos\\fx-v1"},"cost":0,"tokens":{"total":23857,"input":23840,"output":3,"reasoning":14,"cache":{"write":0,"read":0}},"modelID":"big-pickle","providerID":"opencode","time":{"created":1795603003555,"completed":1795603032044},"finish":"stop"}');
CREATE TABLE "migration" (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL);
CREATE TABLE `part` (
          `id` text PRIMARY KEY,
          `message_id` text NOT NULL,
          `session_id` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_part_message_id_message_id_fk` FOREIGN KEY (`message_id`) REFERENCES `message`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `permission` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `action` text NOT NULL,
          `resource` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `fk_permission_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `project` (
          `id` text PRIMARY KEY,
          `worktree` text NOT NULL,
          `vcs` text,
          `name` text,
          `icon_url` text,
          `icon_url_override` text,
          `icon_color` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_initialized` integer,
          `sandboxes` text NOT NULL,
          `commands` text
        , `time_active` integer DEFAULT 0 NOT NULL, `time_archived` integer);
CREATE TABLE `project_directory` (
          `project_id` text NOT NULL,
          `directory` text NOT NULL,
          `type` text,
          `strategy` text,
          `time_created` integer NOT NULL,
          CONSTRAINT `project_directory_pk` PRIMARY KEY(`project_id`, `directory`),
          CONSTRAINT `fk_project_directory_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `session` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `workspace_id` text,
          `parent_id` text,
          `slug` text NOT NULL,
          `directory` text NOT NULL,
          `path` text,
          `title` text NOT NULL,
          `version` text NOT NULL,
          `share_url` text,
          `summary_additions` integer,
          `summary_deletions` integer,
          `summary_files` integer,
          `summary_diffs` text,
          `metadata` text,
          `cost` real DEFAULT 0 NOT NULL,
          `tokens_input` integer DEFAULT 0 NOT NULL,
          `tokens_output` integer DEFAULT 0 NOT NULL,
          `tokens_reasoning` integer DEFAULT 0 NOT NULL,
          `tokens_cache_read` integer DEFAULT 0 NOT NULL,
          `tokens_cache_write` integer DEFAULT 0 NOT NULL,
          `revert` text,
          `permission` text,
          `agent` text,
          `model` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_compacting` integer,
          `time_archived` integer,
          CONSTRAINT `fk_session_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
INSERT INTO "session" VALUES('ses_dedb89c6affe2Bf0J714lbuYfg','c6c908d3347c6472ac7d466627e5fe5e9e2a7075',NULL,NULL,'quick-wolf','<scratch>/repos/fx-v1','','Ok reply request','1.19.0',NULL,0,0,0,NULL,NULL,0.0,23840,3,14,0,0,NULL,'[{"permission":"question","pattern":"*","action":"deny"},{"permission":"plan_enter","pattern":"*","action":"deny"},{"permission":"plan_exit","pattern":"*","action":"deny"}]','build','{"id":"big-pickle","providerID":"opencode","variant":"default"}',1795602998165,1795603032073,NULL,NULL);
CREATE TABLE `session_inbox` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `payload` text NOT NULL,
          `delivery` text NOT NULL,
          `enqueued_seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          CONSTRAINT `fk_session_inbox_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE "session_message" (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_session_message_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
INSERT INTO "session_message" VALUES('msg_212476c8e001NBTO6NuYEtXANQ','ses_dedb89c6affe2Bf0J714lbuYfg','user',0,1795603000462,1795603016720,'{"text":"Reply with the word ok.","time":{"created":1795603000462}}');
INSERT INTO "session_message" VALUES('msg_2124778a3001mTp67wZXE5NuVr','ses_dedb89c6affe2Bf0J714lbuYfg','assistant',1,1795603003555,1795603032046,'{"agent":"build","model":{"providerID":"opencode","id":"big-pickle","variant":"default"},"content":[{"type":"reasoning","text":"The user wants me to reply with the word \"ok\". Simple.","time":{"created":1795603030780,"completed":1795603031178}},{"type":"text","text":"ok"}],"snapshot":{"start":"57b37e28e41047d9d375f22a6144934bb658ccde","end":"57b37e28e41047d9d375f22a6144934bb658ccde"},"finish":"stop","cost":0,"tokens":{"input":23840,"output":3,"reasoning":14,"cache":{"read":0,"write":0}},"time":{"created":1795603003555,"completed":1795603032046}}');
INSERT INTO "session_message" VALUES('msg_212486b7a001NpWyF16cIs5GAt','ses_dedb7a25dffeoEMhN9M3UVV92v','user',4,1795603067977,1795603067978,'{"time":{"created":1795603067977},"text":"Reply with the word ok.","files":[]}');
INSERT INTO "session_message" VALUES('msg_21248748a001RLXEGPpGQxDbS6','ses_dedb7a25dffeoEMhN9M3UVV92v','assistant',7,1795603068058,1795603075902,'{"time":{"created":1795603068058,"firstToken":1795603075165,"streamed":1795603075281,"completed":1795603075900},"agent":"build","model":{"id":"big-pickle","providerID":"opencode","variant":"default"},"content":[{"type":"reasoning","text":"The user asks to reply with the word \"ok\". Simple.","native":{"reasoningField":"reasoning_content"},"time":{"created":1795603075169,"completed":1795603075276}},{"type":"text","text":"ok"}],"snapshot":{"start":"57b37e28e41047d9d375f22a6144934bb658ccde","end":"57b37e28e41047d9d375f22a6144934bb658ccde","files":[]},"finish":"stop","rawFinish":"stop","cost":0,"tokens":{"input":13981,"output":3,"reasoning":13,"cache":{"read":1536,"write":0}}}');
INSERT INTO "session_message" VALUES('msg_212489351001il3dBpIkWrc1Mf','ses_dedb7a25dffeoEMhN9M3UVV92v','idle',15,1795603075921,1795603075922,'{"time":{"created":1795603075921},"outcome":"succeeded"}');
CREATE TABLE `session_part` (
          `message_id` text NOT NULL,
          `session_id` text NOT NULL,
          `ordinal` integer NOT NULL,
          `type` text NOT NULL,
          `call_id` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `session_part_pk` PRIMARY KEY(`message_id`, `ordinal`),
          CONSTRAINT `fk_session_part_message_id_session_message_id_fk` FOREIGN KEY (`message_id`) REFERENCES `session_message`(`id`) ON DELETE CASCADE,
          CONSTRAINT `fk_session_part_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `session_share` (
          `session_id` text PRIMARY KEY,
          `id` text NOT NULL,
          `secret` text NOT NULL,
          `url` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `fk_session_share_session_id_session_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `session_v2` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `workspace_id` text,
          `parent_id` text,
          `fork_session_id` text,
          `fork_boundary` text,
          `slug` text NOT NULL,
          `directory` text NOT NULL,
          `path` text,
          `title` text,
          `version` text NOT NULL,
          `share_url` text,
          `summary_additions` integer,
          `summary_deletions` integer,
          `summary_files` integer,
          `summary_diffs` text,
          `metadata` text,
          `cost` real DEFAULT 0 NOT NULL,
          `tokens_input` integer DEFAULT 0 NOT NULL,
          `tokens_output` integer DEFAULT 0 NOT NULL,
          `tokens_reasoning` integer DEFAULT 0 NOT NULL,
          `tokens_cache_read` integer DEFAULT 0 NOT NULL,
          `tokens_cache_write` integer DEFAULT 0 NOT NULL,
          `revert` text,
          `permission` text,
          `agent` text,
          `model` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_compacting` integer,
          `time_archived` integer,
          `time_suspended` integer, `resume_attempts` integer DEFAULT 0 NOT NULL, `time_idle` integer, `time_viewed` integer, `idle_outcome` text, `time_busy` integer,
          CONSTRAINT `fk_session_v2_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
INSERT INTO "session_v2" VALUES('ses_dedb89c6affe2Bf0J714lbuYfg','c6c908d3347c6472ac7d466627e5fe5e9e2a7075',NULL,NULL,NULL,NULL,'quick-wolf','<scratch>/repos/fx-v1','','Ok reply request','1.19.0',NULL,0,0,0,NULL,NULL,0.0,23840,3,14,0,0,NULL,NULL,'build','{"id":"big-pickle","providerID":"opencode","variant":"default"}',1795602998165,1795603032073,NULL,NULL,NULL,0,NULL,NULL,NULL,NULL);
INSERT INTO "session_v2" VALUES('ses_dedb7a25dffeoEMhN9M3UVV92v','c6c908d3347c6472ac7d466627e5fe5e9e2a7075',NULL,NULL,NULL,NULL,'glowing-tiger','<scratch>/repos/fx-v2u','','Ok reply instruction','2.3.2',NULL,NULL,NULL,NULL,NULL,NULL,0.0,14441,8,114,1536,0,NULL,NULL,NULL,'{"id":"big-pickle","providerID":"opencode"}',1795603065387,1795603075900,NULL,NULL,NULL,0,1795603075921,NULL,'succeeded',1795603067928);
CREATE TABLE `todo` (
          `session_id` text NOT NULL,
          `content` text NOT NULL,
          `status` text NOT NULL,
          `priority` text NOT NULL,
          `position` integer NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `todo_pk` PRIMARY KEY(`session_id`, `position`),
          CONSTRAINT `fk_todo_session_id_session_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session`(`id`) ON DELETE CASCADE
        );
CREATE TABLE "workspace" (
          `id` text PRIMARY KEY,
          `provider` text NOT NULL,
          `binding` text,
          `created_at` integer NOT NULL,
          `last_used_at` integer NOT NULL
        );
CREATE TABLE `worktree` (
          `project_id` text NOT NULL,
          `directory` text NOT NULL,
          `strategy` text,
          `time_created` integer NOT NULL,
          CONSTRAINT `worktree_pk` PRIMARY KEY(`project_id`, `directory`),
          CONSTRAINT `fk_worktree_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
CREATE UNIQUE INDEX `event_aggregate_seq_idx` ON `event` (`aggregate_id`,`seq`);
CREATE INDEX `event_aggregate_type_seq_idx` ON `event` (`aggregate_id`,`type`,`seq`);
CREATE UNIQUE INDEX `permission_project_action_resource_idx` ON `permission` (`project_id`,`action`,`resource`);
CREATE INDEX `message_session_time_created_id_idx` ON `message` (`session_id`,`time_created`,`id`);
CREATE INDEX `part_message_id_id_idx` ON `part` (`message_id`,`id`);
CREATE INDEX `part_session_idx` ON `part` (`session_id`);
CREATE INDEX `session_project_idx` ON `session` (`project_id`);
CREATE INDEX `session_workspace_idx` ON `session` (`workspace_id`);
CREATE INDEX `session_parent_idx` ON `session` (`parent_id`);
CREATE INDEX `todo_session_idx` ON `todo` (`session_id`);
CREATE UNIQUE INDEX `session_message_session_seq_idx` ON `session_message` (`session_id`,`seq`);
CREATE INDEX `session_message_session_type_seq_idx` ON `session_message` (`session_id`,`type`,`seq`);
CREATE INDEX `session_message_session_time_created_id_idx` ON `session_message` (`session_id`,`time_created`,`id`);
CREATE INDEX `session_message_time_created_idx` ON `session_message` (`time_created`);
CREATE INDEX `session_v2_project_idx` ON `session_v2` (`project_id`);
CREATE INDEX `session_v2_workspace_idx` ON `session_v2` (`workspace_id`);
CREATE INDEX `session_v2_parent_idx` ON `session_v2` (`parent_id`);
CREATE INDEX `session_v2_time_suspended_idx` ON `session_v2` (`time_suspended`) WHERE "session_v2"."time_suspended" is not null;
CREATE INDEX `session_inbox_session_delivery_seq_idx` ON `session_inbox` (`session_id`,`delivery`,`enqueued_seq`);
CREATE UNIQUE INDEX `session_inbox_session_enqueued_seq_idx` ON `session_inbox` (`session_id`,`enqueued_seq`);
CREATE INDEX `blob_ref_hash_idx` ON `blob_ref` (`hash`);
CREATE INDEX `blob_ref_session_seq_idx` ON `blob_ref` (`session_id`,`seq`);
CREATE INDEX `session_part_session_message_idx` ON `session_part` (`session_id`,`message_id`,`ordinal`);
CREATE INDEX `job_session_idx` ON `job` (`session_id`);
CREATE INDEX `job_child_session_idx` ON `job` (`child_session_id`);
CREATE INDEX `job_id_idx` ON `job` (`id`);
CREATE INDEX `automation_project_idx` ON `automation` (`project_id`);
CREATE INDEX `automation_session_idx` ON `automation` (`session_id`);
CREATE INDEX `automation_active_idx` ON `automation` (`next_due_at`) WHERE "automation"."status" = 'active';
COMMIT;

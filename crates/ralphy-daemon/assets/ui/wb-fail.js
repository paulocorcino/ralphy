/* ---------------------------------------------------------------------------
   Verb-failure presenter — the single "is this reply a failure, and what does
   it say?" adapter (#207).

   Every daemon verb reply is either a one-shot Query/Mutate/Observe reply
   (`{status:"ok"}` or `{status:"error", message|reason}`) or a `spawn` stream
   frame (`output`/`exited`/`error`). `isError` treats ONLY `status === "error"`
   (and a missing reply) as a failure, so a `spawn` stream's `output`/`exited`
   frames are never misread as errors. `message` extracts the verbatim
   `message`/`reason` the daemon sent, never inventing text.
--------------------------------------------------------------------------- */
(function () {
  function isError(reply) {
    return !reply || reply.status === "error";
  }
  function message(reply, fallback) {
    return (reply && (reply.message || reply.reason)) || fallback || "refused";
  }
  window.WBFail = { isError, message };
})();

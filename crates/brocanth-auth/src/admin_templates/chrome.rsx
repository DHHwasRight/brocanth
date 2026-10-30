// What both screens open with: the panel's stylesheet, and the sentence a
// screen has to say when the accounts on it are not kept anywhere.
//
// The sheet is inlined rather than served: the admin answers with a page and
// mounts no asset route, and a panel is in no position to add one. It carries
// no preflight — the admin's own sheet already reset the document — only the
// theme variables and the utilities these templates name.
//
// The notice is not decoration. A screen that looks like a user database
// while being a `HashMap` behind a `Mutex` (`MemoryStore`) has to admit it, on the screen, where somebody
// about to end a session can read it. The store answers
// `AccountDirectory::is_durable`; the words are the panel's.

pub fn Chrome(volatile: bool) -> Element {
    <style>{! crate::admin::STYLE }</style>

    if volatile {
        <div class="mb-5 rounded-card border border-line bg-stage px-3.5 py-3 text-sm text-ink-soft"
             role="note">
            <strong class="font-semibold text-ink">These accounts are held in memory.</strong>
            <span class="block mt-1">
                They are gone when this process restarts, and no other process serving
                the same application can see them. PgStore, behind the postgres feature
                of brocanth-auth, keeps them in Postgres, and this notice then goes away.
            </span>
        </div>
    }
}

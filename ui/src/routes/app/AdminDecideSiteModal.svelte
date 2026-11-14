<script lang="ts">
  import FactsList from '@/components/admin/FactsList.svelte'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import SavedMark from '@/components/admin/SavedMark.svelte'
  import { tableCell, tableHead, tableRow } from '@/components/admin/table'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import { cn } from '@/lib/cn'
  import { postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import { PRIMITIVE_LABEL, siteFacts, type DecideData, type LedgerRow, type Site, type SiteShadow } from './admin-decide'

  // One place a decision model is asked something: what it does, the
  // threshold it acts at, and what its own traffic measured.
  let {
    site: s,
    shadow: m,
    configured,
    speechRooms,
    floorSaved,
    onFloor,
    onClose,
  }: {
    site: Site
    shadow: SiteShadow | undefined
    configured: boolean
    speechRooms: DecideData['speechRooms']
    /** The threshold saves when its field is left, and a save just landed. */
    floorSaved: boolean
    onFloor: (floor: number) => void
    onClose: () => void
  } = $props()

  // THE ROWS BEHIND THE PERCENTAGE. "Agreed on 94% of 300" is evidence about a
  // population; a person deciding whether to trust a judgment needs the cases
  // — and specifically the disagreements, because whether those were the model
  // being right or the model being wrong is the whole question.
  let showing = $state(false)
  let rows = $state<LedgerRow[]>([])
  let loadingRows = $state(false)
  const showRows = async () => {
    if (showing) {
      showing = false
      return
    }
    showing = true
    rows = []
    loadingRows = true
    try {
      const r = await postJson<{ rows: LedgerRow[] }>('/api/admin/decide', { action: 'rows', site: s.id, limit: 25 })
      rows = r.rows ?? []
    } catch (e) {
      toastError('Could not read that site’s ledger', e)
      showing = false
    } finally {
      loadingRows = false
    }
  }

  // THE SECOND GATE of unprompted replies, which is per-room by necessity.
  // "Which rooms would start talking if I flip this" is the question somebody
  // asks immediately before flipping it. Read-only: the switch stays in each
  // room's own settings.
  const speaking = $derived(speechRooms.filter((r) => r.agents > 0))
  const text = 'font-sans text-[13px] leading-5 text-muted'
  const mono = 'font-mono text-xs'
</script>

<Modal open {onClose} width="max-w-2xl">
  {#snippet title()}
    <span class="flex items-center gap-2 font-sans text-[15px] font-semibold leading-5 text-fg">
      {s.label}<Chip>{PRIMITIVE_LABEL[s.primitive] ?? s.primitive}</Chip>
    </span>
  {/snippet}
  <div class="space-y-5">
    <p class={text}>{s.acts}</p>

    {#if s.switchLivesAt}
      <!-- Named rather than offered twice: two spellings of one switch is how
           the two come to disagree. -->
      <p class={text}>Turned on from {s.switchLivesAt}.</p>
    {:else}
      <SettingsField
        label="Threshold"
        for="decide-floor-{s.id}"
        hint={s.on
          ? `Acts when the ${s.reads === 'lean' ? 'probability of yes' : 'certainty'} is at least this. The default is ${s.defaultFloor}.`
          : 'Turn this decision on to change its threshold.'}
        info={s.floorWhy}
      >
        <div class="flex items-center gap-2">
          <Input
            id="decide-floor-{s.id}"
            size="sm"
            type="number"
            step="0.05"
            min="0"
            max="1"
            disabled={!s.on}
            value={String(s.floor)}
            onblur={(e) => Number(e.currentTarget.value) !== s.floor && onFloor(Number(e.currentTarget.value))}
            class="w-28 tabular-nums"
          />
          {#if s.on && s.floor !== s.defaultFloor}
            <Button size="md" variant="ghost" onclick={() => onFloor(s.defaultFloor)}>Reset to {s.defaultFloor}</Button>
          {/if}
          <SavedMark on={floorSaved} />
        </div>
      </SettingsField>
    {/if}

    {#if s.id === 'channel-speech'}
      <p class={text}>
        {#if speechRooms.length === 0}
          No channel has its own agent initiative switch on, so nothing would speak yet.
        {:else}
          {speaking.length} of {speechRooms.length}
          {speechRooms.length === 1 ? 'channel allows' : 'channels allow'} this and {speaking.length === 1 ? 'has' : 'have'} an agent:
          {#each speaking.slice(0, 8) as r, i (r.id)}<span class={cn(mono, 'text-fg')}>#{r.name}</span>{#if i < Math.min(speaking.length, 8) - 1}{', '}{/if}{/each}{#if speaking.length > 8}, and more{/if}.
          Each channel's own switch is in its settings.
        {/if}
      </p>
    {/if}

    <!-- SHADOW MODE — what the model WOULD have decided, beside what the
         existing code did decide. A site that has not switched over runs its
         own path and the model is asked the same question with nobody
         listening, so a threshold can be chosen from this install's own
         traffic rather than from a vendor's published numbers. -->
    {#if m || s.measuredAt || !s.switchLivesAt}
    <div class="border-t border-line-subtle pt-5">
      <h3 class="font-sans text-[13px] font-medium leading-5 text-fg">Measured on this instance</h3>
      {#if m}
        <FactsList facts={siteFacts(m)} class="px-0 pb-0 pt-3" />
        <div class="mt-3">
          {#if m.judgments > m.calibrated}
            <!-- Counted over the rows that carried a SINGLE judgment. A derived
                 row — the tool keep-set, the brief's reorder — has no one
                 probability behind it, so it is not counted as missing one. -->
            <NoticeRow status="warn" class="px-0 py-1.5">
              {(m.judgments - m.calibrated).toLocaleString()} of {m.judgments.toLocaleString()} answers carried no confidence.
            </NoticeRow>
          {/if}
          {#if m.models.length > 1}
            <!-- TWO MODELS AT ONE SITE, which makes the rate above about
                 neither of them. An alias creates this silently the day it
                 starts pointing somewhere new, which is why the ledger records
                 the model that ANSWERED rather than the one we asked for. -->
            <NoticeRow status="warn" class="px-0 py-1.5">
              {m.models.length} models answered here, so the agreement rate describes neither:
              {#each m.models as mm, i (mm.model)}<span class={cn(mono, 'text-fg')}>{mm.model}</span> ({mm.judgments.toLocaleString()}){#if i < m.models.length - 1}{', '}{/if}{/each}.
            </NoticeRow>
          {/if}
          {#if m.meanCertaintyWhenDisagreed !== null}
            <NoticeRow status="idle" class="px-0 py-1.5">
              When it disagreed, its mean certainty was
              <span class="tabular-nums text-fg">{m.meanCertaintyWhenDisagreed.toFixed(2)}</span>. Low means a threshold can
              filter those out. High means read them before turning this on.
            </NoticeRow>
          {/if}
        </div>
        {#if m.compared + m.acted > 0}
          <Button size="sm" variant="outline" class="mt-3" onclick={() => void showRows()}>
            {showing ? 'Hide recent decisions' : 'Show recent decisions'}
          </Button>
        {/if}
        {#if showing}
          <div transition:slide={{ duration: 150 }} class="mt-3">
            {#if loadingRows}
              <p class={text}>Reading the ledger.</p>
            {:else if rows.length === 0}
              <p class={text}>Nothing recorded.</p>
            {:else}
              <!-- Disagreements first: they are what is being audited. The
                   SUBJECT is a reference, never the text — a ticket message is
                   somebody's words and this ledger is the wrong place to
                   accumulate them. -->
              <div class="overflow-x-auto rounded-md border border-line">
                <table class="w-full">
                  <thead>
                    <tr>
                      <th class={tableHead}>Subject</th>
                      <th class={tableHead}>Was</th>
                      <th class={tableHead}>Judged</th>
                      <th class={cn(tableHead, 'text-right')}>Certainty</th>
                      <th class={tableHead}>Model</th>
                      <th class={tableHead}>When</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each rows as r, i (`${r.at}-${i}`)}
                      <tr class={tableRow}>
                        <td class={cn(tableCell, 'h-9 max-w-[12rem] truncate', mono)}>
                          <span class="flex items-center gap-2">
                            <span class={cn('h-1.5 w-1.5 shrink-0 rounded-full', r.agreed === false ? 'bg-warning' : 'bg-line')}></span>
                            <span class="truncate">{r.subjectRef ?? '—'}</span>
                          </span>
                        </td>
                        <td class={cn(tableCell, 'h-9', mono)}>{r.baseline ?? '—'}</td>
                        <td class={cn(tableCell, 'h-9', mono)}>{r.portAnswer ?? 'No answer'}</td>
                        <td class={cn(tableCell, 'h-9 text-right tabular-nums')}>
                          {r.certainty === null ? '—' : r.certainty.toFixed(2)}{r.calibrated ? '' : '*'}
                        </td>
                        <td class={cn(tableCell, 'h-9 max-w-[10rem] truncate text-muted', mono)}>{r.model ?? '—'}</td>
                        <td class={cn(tableCell, 'h-9 whitespace-nowrap text-muted', mono)}>{r.at?.slice(0, 16) ?? '—'}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
              <p class={cn(text, 'mt-2')}>
                A gold dot marks a disagreement. A certainty marked * carried no confidence. "No answer" still counts in the total.
              </p>
            {/if}
          </div>
        {/if}
      {:else if s.measuredAt}
        <!-- Not an empty ledger: this site has nothing to compare against, and
             says where its numbers are instead. -->
        <p class={cn(text, 'mt-2')}>Measured as {s.measuredAt}.</p>
      {:else}
        <p class={cn(text, 'mt-2')}>No comparisons recorded yet{configured ? '' : ', because no decision model is set up'}.</p>
      {/if}
    </div>
    {/if}
  </div>
  {#snippet footer()}
    <div class="flex justify-end">
      <Button size="sm" variant="outline" onclick={onClose}>Done</Button>
    </div>
  {/snippet}
</Modal>

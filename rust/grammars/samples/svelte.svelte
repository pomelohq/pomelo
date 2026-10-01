<script>
  // A counter.
  let count = 0;
  const increment = () => (count += 1);
</script>

<button on:click={increment} class="primary">
  Clicked {count} {count === 1 ? "time" : "times"}
</button>

{#if count > 3}
  <p>That is a lot.</p>
{/if}

<style>
  .primary { color: tomato; }
</style>

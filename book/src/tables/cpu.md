## CPU

The CPU is the central component of the zkEVM. Like any CPU, it reads
instructions, executes them and modifies the state (registers and the
memory) accordingly. The constraining of some complex instructions (e.g.
Keccak hashing) is delegated to other tables. This section will only
briefly present the CPU and its columns. Details about the CPU logic
will be provided later.

### CPU flow

An execution run can be decomposed into two distinct parts:

-   **CPU cycles:** The bulk of the execution. In each row, the CPU
    reads the current code at the program counter (PC) address, and
    executes it. The current code can be the kernel code, or whichever
    code is being executed in the current context (transaction code or
    contract code). Executing an instruction consists in modifying the
    registers, possibly performing some memory operations, and updating
    the PC.

-   **Padding:** At the end of the execution, we need to pad the length
    of the CPU trace to the next power of two. When the program counter
    reaches the special halting label in the kernel, execution halts.
    Constraints ensure that every subsequent row is a padding row and
    that execution cannot resume.

In the CPU cycles phase, the CPU can switch between different contexts,
which correspond to the different environments of the possible calls.
Context 0 is the kernel itself, which handles initialization (input
processing, transaction parsing, transaction trie updating\...) and
termination (receipt creation, final trie checks\...) before and after
executing the transaction. Subsequent contexts are created when
executing user code (transaction or contract code). In a non-zero user
context, syscalls may be executed, which are specific instructions
written in the kernel. They don't change the context but change the code
context, which is where the instructions are read from.

#### Continuations

A full run of the zkEVM consists in initializing the zkEVM with the
input state, executing a certain number of transactions, and then
validating the output state. However, for performance reasons, a run is
split in multiple segments of at most `MAX_CPU_CYCLES` cycles, which can
be proven individually. Continuations ensure that the segments are part
of the same run and guarantees that the state at the end of a segment is
equal to the state at the beginning of the next.

The state to propagate from one segment to another contains some of the
zkEVM registers plus the current memory. These registers are stored in
memory as dedicated global metadata, and the memory to propagate is
stored in two STARK tables: `MemBefore` and `MemAfter`. To check the
consistency of the memory, the Merkle cap of the previous `MemAfter` is
compared to the Merkle cap of the next `MemBefore`.

### CPU columns

#### Registers

-   `context`: Indicates which context we are in. 0 for the kernel, and
    a positive integer for every user context. Incremented by 1 at every
    call.

-   `code_context`: Indicates in which context the code to execute
    resides. It's equal to `context` in user mode, but is always 0 in
    kernel mode.

-   `program_counter`: The address of the instruction to be read and
    executed.

-   `stack_len`: The current length of the stack.

-   `is_kernel_mode`: Boolean indicating whether we are in kernel (i.e.
    privileged) mode. This means we are executing kernel code, and we
    have access to privileged instructions.

-   `gas`: The current amount of gas used in the current context. It is
    eventually checked to be below the current gas limit. Must fit in 32
    bits.

-   `clock`: Monotonic counter which starts at 0 and is incremented by 1
    at each row. Used to enforce correct ordering of memory accesses.

-   `opcode_bits`: 8 boolean columns, which are the bit decomposition of
    the opcode being read at the current PC.

#### Operation flags

Boolean flags. During CPU cycles phase, each row executes a single
instruction, which sets one and only one operation flag. No flag is set
during padding. The decoding constraints ensure that the flag set
corresponds to the opcode being read. There isn't a 1-to-1
correspondence between instructions and flags. For efficiency, the same
flag can be set by different, unrelated instructions (e.g. `eq_iszero`,
which represents the `EQ` and the `ISZERO` instructions). When there is
a need to differentiate them in constraints, we filter them with their
respective opcode: since the first bit of `EQ`'s opcode (resp.
`ISZERO`'s opcode) is 0 (resp. 1), we can filter a constraint for an EQ
instruction with `eq_iszero * (1 - opcode_bits[0])` (resp.
`eq_iszero * opcode_bits[0]`).

#### Memory columns

The CPU interacts with the EVM memory via its memory channels. At each
row, a memory channel can execute a write, a read, or be disabled. A
full memory channel is composed of:

-   `used`: Boolean flag. If it's set to 1, a memory operation is
    executed in this channel at this row. If it's set to 0, no operation
    is done but its columns might be reused for other purposes.

-   `is_read`: Boolean flag indicating if a memory operation is a read
    or a write.

-   3 `address` columns. A memory address is made of three parts:
    `context`, `segment` and `virtual`.

-   8 `value` columns. EVM words are 256 bits long, and they are broken
    down in 8 32-bit limbs.

The last memory channel is a partial channel: it doesn't have its own
`value` columns and shares them with the first full memory channel. This
allows us to save eight columns.

#### General columns

There are 8 shared general columns. Depending on the instruction, they
are used differently:

-   `Exceptions`: When raising an exception, the first three general
    columns are the bit decomposition of the exception code. They are
    used to jump to the correct exception handler.

-   `Logic`: For EQ, and ISZERO operations, it's easy to check that the
    result is 1 if `input0` and `input1` are equal. It's more difficult
    to prove that, if the result is 0, the inputs are actually unequal.
    To prove it, each general column contains the modular inverse of
    $(\texttt{input0}_i - \texttt{input1}_i)$ for each limb $i$ (or 0 if
    the limbs are equal). Then the quantity
    $\texttt{general}_i * (\texttt{input0}_i - \texttt{input1}_i)$ will
    be 1 if and only if $\texttt{general}_i$ is indeed the modular
    inverse, which is only possible if the difference is non-zero.

-   `Jumps`: For jumps, we use the first two columns: `should_jump` and
    `cond_sum_pinv`. `should_jump` conditions whether the EVM should
    jump: it's 1 for a JUMP, and $\texttt{condition} \neq 0$ for a
    JUMPI. To check if the condition is actually non-zero for a JUMPI,
    `cond_sum_pinv` stores the modular inverse of `condition` (or 0 if
    it's zero).

-   `Shift`: For shifts, the logic differs depending on whether the
    displacement is lower than $2^{32}$, i.e. if it fits in a single
    value limb. To check if this is not the case, we must check that at
    least one of the seven high limbs is not zero. The general column
    `high_limb_sum_inv` holds the modular inverse of the sum of the
    seven high limbs, and is used to check it's non-zero like the
    previous cases. Contrary to the logic operations, we do not need to
    check limbs individually: each limb has been range-checked to 32
    bits, meaning that it's not possible for the sum to overflow and be
    zero if some of the limbs are non-zero.

-   `Stack`: `stack_inv`, `stack_inv_aux` and `stack_inv_aux_2` are used
    by popping-only (resp. pushing-only) instructions to check if the
    stack is empty after (resp. was empty before) the instruction.
    `stack_len_bounds_ aux` is used to check that the stack doesn't
    overflow in user mode. We use the last four columns to prevent
    conflicts with the other general columns. See
    [stack handling](./../cpu_execution/stack_handling.md) for more details.

-   `Push`: `is_not_kernel` is used to skip range-checking the output of
    a PUSH operation when we are in privileged mode, as the kernel code
    is known and trusted.

-   `Context pruning`: When `SET_CONTEXT` is called to return to a
    parent context, this makes the current context stale. The kernel
    indicates it by setting one general column to 1. For more details
    about context pruning, see [context-pruning](./memory.md#context-pruning).
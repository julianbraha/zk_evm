## Logic

Each row of the logic table corresponds to one bitwise logic operation:
either AND, OR or XOR. Each input for these operations is represented as
256 bits, while the output is stored as eight 32-bit limbs.

Each row therefore contains the following columns:

1.  $f_{\texttt{and}}$, an "is and" flag, which should be 1 for an OR
    operation and 0 otherwise,

2.  $f_{\texttt{or}}$, an "is or" flag, which should be 1 for an OR
    operation and 0 otherwise,

3.  $f_{\texttt{xor}}$, an "is xor" flag, which should be 1 for a XOR
    operation and 0 otherwise,

4.  256 columns $x_{1, i}$ for the bits of the first input $x_1$,

5.  256 columns $x_{2, i}$ for the bits of the second input $x_2$,

6.  8 columns $r_i$ for the 32-bit limbs of the output $r$.

Note that we need all three flags because we need to be able to
distinguish between an operation row and a padding row -- where all
flags are set to 0.

The subdivision into bits is required for the two inputs as the table
carries out bitwise operations. The result, on the other hand, is
represented in 32-bit limbs since we do not need individual bits and can
therefore save the remaining 248 columns. Moreover, the output is
checked against the cpu, which stores values in the same way.

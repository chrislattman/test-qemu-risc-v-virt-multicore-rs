ENTRY(_start)

/* IMAGE_BASE, REGION_BASE and REGION_SIZE are set for each hart by build.rs (--defsym) */

SECTIONS
{
    . = IMAGE_BASE;

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : { *(.rodata .rodata.* .srodata .srodata.*) }
    .eh_frame : { KEEP(*(.eh_frame)) } /* Unwind info, used by debuggers for backtraces */

    .data : {
        *(.data .data.*)
        /* gp points 2KB into the small data, so the 12-bit signed offsets of gp-relative
           accesses can reach 4KB of .sdata/.sbss around it */
        __global_pointer$ = . + 0x800;
        *(.sdata .sdata.*)
    }

    /* Zeroed by startup.S */
    .bss (NOLOAD) : {
        . = ALIGN(16);
        __bss_start = .;
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    . = ALIGN(16);
    . += 0x4000; /* Allocate 16KB stack space */
    _stack_top = .;

    /* Everything, including the stack, must stay inside this hart's private region */
    ASSERT(_stack_top <= REGION_BASE + REGION_SIZE, "image doesn't fit in this hart's private region")
}

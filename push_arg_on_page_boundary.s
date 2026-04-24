	.text
	.globl _foo
_foo:
	pushq %rbp
	movq %rsp, %rbp
	subq $32, %rsp
	movl %edi, -4(%rbp)
	movl %esi, -8(%rbp)
	movl %edx, -12(%rbp)
	movl %ecx, -16(%rbp)
	movl %r8d, -20(%rbp)
	movl %r9d, -24(%rbp)
	movl 16(%rbp), %r10d
	movl %r10d, -28(%rbp)
	movl -28(%rbp), %r10d
	movl %r10d, -32(%rbp)
	addl $1, -32(%rbp)
	movl -32(%rbp), %eax
	addq $32, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $32, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	.text
	.globl _main
_main:
	pushq %rbp
	movq %rsp, %rbp
	subq $16, %rsp
	subq $8, %rsp
	pushq _zed(%rip)
	movl $0, %edi
	movl $0, %esi
	movl $0, %edx
	movl $0, %ecx
	movl $0, %r8d
	movl $0, %r9d
	call _foo
	addq $16, %rsp
	movl %eax, -4(%rbp)
	movl -4(%rbp), %eax
	addq $16, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $16, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret

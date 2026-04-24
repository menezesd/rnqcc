	.text
	.globl _subscript_pp
_subscript_pp:
	pushq %rbp
	movq %rsp, %rbp
	subq $64, %rsp
	movq %rdi, -8(%rbp)
	movq $0, -16(%rbp)
	movq -8(%rbp), %rax
	movq -16(%rbp), %rdx
	leaq (%rax, %rdx, 8), %r10
	movq %r10, -24(%rbp)
	movq -24(%rbp), %r11
	movq (%r11), %r10
	movq %r10, -32(%rbp)
	movq $0, -40(%rbp)
	movq -32(%rbp), %rax
	movq -40(%rbp), %rdx
	leaq (%rax, %rdx, 4), %r10
	movq %r10, -48(%rbp)
	movq -48(%rbp), %r11
	movl (%r11), %r10d
	movl %r10d, -52(%rbp)
	movl -52(%rbp), %eax
	addq $64, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $64, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	.text
	.globl _main
_main:
	pushq %rbp
	movq %rsp, %rbp
	subq $48, %rsp
	movl $42, -4(%rbp)
	leaq -4(%rbp), %r10
	movq %r10, -16(%rbp)
	movq -16(%rbp), %r10
	movq %r10, -24(%rbp)
	leaq -24(%rbp), %r10
	movq %r10, -32(%rbp)
	movq -32(%rbp), %r10
	movq %r10, -40(%rbp)
	movq -40(%rbp), %rdi
	call _subscript_pp
	movl %eax, -44(%rbp)
	movl -44(%rbp), %eax
	addq $48, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $48, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret

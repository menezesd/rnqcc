	.text
	.globl _main
_main:
	pushq %rbp
	movq %rsp, %rbp
	subq $320, %rsp
	movsd ___double_const_0(%rip), %xmm14
	movsd %xmm14, -8(%rbp)
	movsd -8(%rbp), %xmm14
	movsd %xmm14, -16(%rbp)
	movsd -16(%rbp), %xmm14
	divsd _zero.0(%rip), %xmm14
	movsd %xmm14, -16(%rbp)
	movsd -16(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd ___double_const_1(%rip), %xmm14
	movsd %xmm14, -32(%rbp)
	movsd -24(%rbp), %xmm14
	comisd -32(%rbp), %xmm14
	movl $0, -36(%rbp)
	setb -36(%rbp)
	cmpl $0, -36(%rbp)
	jne .Lor_true.6
	movsd ___double_const_2(%rip), %xmm14
	movsd %xmm14, -48(%rbp)
	movsd -24(%rbp), %xmm14
	comisd -48(%rbp), %xmm14
	movl $0, -52(%rbp)
	sete -52(%rbp)
	cmpl $0, -52(%rbp)
	jne .Lor_true.6
	movl $0, -56(%rbp)
	jmp .Lor_end.7
.Lor_true.6:
	movl $1, -56(%rbp)
.Lor_end.7:
	cmpl $0, -56(%rbp)
	jne .Lor_true.4
	movsd ___double_const_3(%rip), %xmm14
	movsd %xmm14, -64(%rbp)
	movsd -24(%rbp), %xmm14
	comisd -64(%rbp), %xmm14
	movl $0, -68(%rbp)
	seta -68(%rbp)
	cmpl $0, -68(%rbp)
	jne .Lor_true.4
	movl $0, -72(%rbp)
	jmp .Lor_end.5
.Lor_true.4:
	movl $1, -72(%rbp)
.Lor_end.5:
	cmpl $0, -72(%rbp)
	jne .Lor_true.2
	movsd ___double_const_4(%rip), %xmm14
	movsd %xmm14, -80(%rbp)
	movsd -24(%rbp), %xmm14
	comisd -80(%rbp), %xmm14
	movl $0, -84(%rbp)
	setbe -84(%rbp)
	cmpl $0, -84(%rbp)
	jne .Lor_true.2
	movl $0, -88(%rbp)
	jmp .Lor_end.3
.Lor_true.2:
	movl $1, -88(%rbp)
.Lor_end.3:
	cmpl $0, -88(%rbp)
	jne .Lor_true.0
	movsd ___double_const_5(%rip), %xmm14
	movsd %xmm14, -96(%rbp)
	movsd -24(%rbp), %xmm14
	comisd -96(%rbp), %xmm14
	movl $0, -100(%rbp)
	setae -100(%rbp)
	cmpl $0, -100(%rbp)
	jne .Lor_true.0
	movl $0, -104(%rbp)
	jmp .Lor_end.1
.Lor_true.0:
	movl $1, -104(%rbp)
.Lor_end.1:
	cmpl $0, -104(%rbp)
	je .Lif_end.8
	movl $1, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.8:
	movl $1, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -112(%rbp)
	movsd -112(%rbp), %xmm14
	comisd -24(%rbp), %xmm14
	movl $0, -116(%rbp)
	setb -116(%rbp)
	cmpl $0, -116(%rbp)
	jne .Lor_true.15
	movl $1, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -128(%rbp)
	movsd -128(%rbp), %xmm14
	comisd -24(%rbp), %xmm14
	movl $0, -132(%rbp)
	sete -132(%rbp)
	cmpl $0, -132(%rbp)
	jne .Lor_true.15
	movl $0, -136(%rbp)
	jmp .Lor_end.16
.Lor_true.15:
	movl $1, -136(%rbp)
.Lor_end.16:
	cmpl $0, -136(%rbp)
	jne .Lor_true.13
	movl $1, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -144(%rbp)
	movsd -144(%rbp), %xmm14
	comisd -24(%rbp), %xmm14
	movl $0, -148(%rbp)
	seta -148(%rbp)
	cmpl $0, -148(%rbp)
	jne .Lor_true.13
	movl $0, -152(%rbp)
	jmp .Lor_end.14
.Lor_true.13:
	movl $1, -152(%rbp)
.Lor_end.14:
	cmpl $0, -152(%rbp)
	jne .Lor_true.11
	movl $1, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -160(%rbp)
	movsd -160(%rbp), %xmm14
	comisd -24(%rbp), %xmm14
	movl $0, -164(%rbp)
	setbe -164(%rbp)
	cmpl $0, -164(%rbp)
	jne .Lor_true.11
	movl $0, -168(%rbp)
	jmp .Lor_end.12
.Lor_true.11:
	movl $1, -168(%rbp)
.Lor_end.12:
	cmpl $0, -168(%rbp)
	jne .Lor_true.9
	movl $1, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -176(%rbp)
	movsd -176(%rbp), %xmm14
	comisd -24(%rbp), %xmm14
	movl $0, -180(%rbp)
	setae -180(%rbp)
	cmpl $0, -180(%rbp)
	jne .Lor_true.9
	movl $0, -184(%rbp)
	jmp .Lor_end.10
.Lor_true.9:
	movl $1, -184(%rbp)
.Lor_end.10:
	cmpl $0, -184(%rbp)
	je .Lif_end.17
	movl $2, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.17:
	movsd -24(%rbp), %xmm14
	comisd -24(%rbp), %xmm14
	movl $0, -188(%rbp)
	sete -188(%rbp)
	cmpl $0, -188(%rbp)
	je .Lif_end.18
	movl $3, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.18:
	movsd -24(%rbp), %xmm14
	comisd -24(%rbp), %xmm14
	movl $0, -192(%rbp)
	setne -192(%rbp)
	cmpl $0, -192(%rbp)
	movl $0, -196(%rbp)
	sete -196(%rbp)
	cmpl $0, -196(%rbp)
	je .Lif_end.19
	movl $4, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.19:
	movsd -24(%rbp), %xmm0
	call _double_isnan
	movl %eax, -200(%rbp)
	cmpl $0, -200(%rbp)
	movl $0, -204(%rbp)
	sete -204(%rbp)
	cmpl $0, -204(%rbp)
	je .Lif_end.20
	movl $5, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.20:
	movl $4, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -216(%rbp)
	movsd -216(%rbp), %xmm14
	movsd %xmm14, -224(%rbp)
	movsd -224(%rbp), %xmm14
	mulsd -24(%rbp), %xmm14
	movsd %xmm14, -224(%rbp)
	movsd -224(%rbp), %xmm0
	call _double_isnan
	movl %eax, -228(%rbp)
	cmpl $0, -228(%rbp)
	movl $0, -232(%rbp)
	sete -232(%rbp)
	cmpl $0, -232(%rbp)
	je .Lif_end.21
	movl $6, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.21:
	movsd ___double_const_6(%rip), %xmm14
	movsd %xmm14, -240(%rbp)
	movsd -240(%rbp), %xmm14
	movsd %xmm14, -248(%rbp)
	movsd -248(%rbp), %xmm14
	divsd -24(%rbp), %xmm14
	movsd %xmm14, -248(%rbp)
	movsd -248(%rbp), %xmm0
	call _double_isnan
	movl %eax, -252(%rbp)
	cmpl $0, -252(%rbp)
	movl $0, -256(%rbp)
	sete -256(%rbp)
	cmpl $0, -256(%rbp)
	je .Lif_end.22
	movl $7, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.22:
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -264(%rbp)
	movsd -264(%rbp), %xmm14
	xorpd ___double_const_7(%rip), %xmm14
	movsd %xmm14, -264(%rbp)
	movsd -264(%rbp), %xmm0
	call _double_isnan
	movl %eax, -268(%rbp)
	cmpl $0, -268(%rbp)
	movl $0, -272(%rbp)
	sete -272(%rbp)
	cmpl $0, -272(%rbp)
	je .Lif_end.23
	movl $8, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.23:
	xorpd %xmm14, %xmm14
	comisd -24(%rbp), %xmm14
	movl $0, -276(%rbp)
	sete -276(%rbp)
	cmpl $0, -276(%rbp)
	je .Lif_end.24
	movl $9, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.24:
	xorpd %xmm14, %xmm14
	comisd -24(%rbp), %xmm14
	je .Lif_else.25
	jmp .Lif_end.26
.Lif_else.25:
	movl $10, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.26:
	movl $0, -280(%rbp)
.Lstart_loop.0:
	xorpd %xmm14, %xmm14
	comisd -24(%rbp), %xmm14
	je .Lbreak_loop.0
	movl $1, -280(%rbp)
	jmp .Lbreak_loop.0
.Lcontinue_loop.0:
	jmp .Lstart_loop.0
.Lbreak_loop.0:
	cmpl $0, -280(%rbp)
	movl $0, -284(%rbp)
	sete -284(%rbp)
	cmpl $0, -284(%rbp)
	je .Lif_end.27
	movl $11, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.27:
	movl $0, -280(%rbp)
.Lcontinue_loop.1:
	xorpd %xmm14, %xmm14
	comisd -24(%rbp), %xmm14
	je .Lbreak_loop.1
	movl $1, -280(%rbp)
	jmp .Lbreak_loop.1
	jmp .Lcontinue_loop.1
.Lbreak_loop.1:
	cmpl $0, -280(%rbp)
	movl $0, -288(%rbp)
	sete -288(%rbp)
	cmpl $0, -288(%rbp)
	je .Lif_end.28
	movl $12, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.28:
	movl $1, -292(%rbp)
	negl -292(%rbp)
	movl -292(%rbp), %r10d
	movl %r10d, -280(%rbp)
.Lstart_loop.2:
	movl -280(%rbp), %r10d
	movl %r10d, -296(%rbp)
	addl $1, -296(%rbp)
	movl -296(%rbp), %r10d
	movl %r10d, -280(%rbp)
	cmpl $0, -280(%rbp)
	je .Lif_end.29
	jmp .Lbreak_loop.2
.Lif_end.29:
.Lcontinue_loop.2:
	xorpd %xmm14, %xmm14
	comisd -24(%rbp), %xmm14
	jne .Lstart_loop.2
.Lbreak_loop.2:
	cmpl $0, -280(%rbp)
	movl $0, -300(%rbp)
	sete -300(%rbp)
	cmpl $0, -300(%rbp)
	je .Lif_end.30
	movl $13, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.30:
	xorpd %xmm14, %xmm14
	comisd -24(%rbp), %xmm14
	je .Lcond_else.31
	movl $1, -304(%rbp)
	jmp .Lcond_end.32
.Lcond_else.31:
	movl $0, -308(%rbp)
	jmp .Lcond_end2.33
.Lcond_end.32:
	movl -304(%rbp), %r10d
	movl %r10d, -308(%rbp)
.Lcond_end2.33:
	movl -308(%rbp), %r10d
	movl %r10d, -280(%rbp)
	cmpl $0, -280(%rbp)
	movl $0, -312(%rbp)
	sete -312(%rbp)
	cmpl $0, -312(%rbp)
	je .Lif_end.34
	movl $14, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.34:
	movl $0, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $320, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	.bss
	.balign 16
_zero.0:
	.zero 8
	.data
	.balign 16
___double_const_0:
	.quad 0
	.data
	.balign 16
___double_const_1:
	.quad 0
	.data
	.balign 16
___double_const_2:
	.quad 0
	.data
	.balign 16
___double_const_3:
	.quad 0
	.data
	.balign 16
___double_const_4:
	.quad 0
	.data
	.balign 16
___double_const_5:
	.quad 0
	.data
	.balign 16
___double_const_6:
	.quad 4657056266235936768
	.data
	.balign 16
___double_const_7:
	.quad 9223372036854775808

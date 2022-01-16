use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::{Buf, Bytes};

use crate::client::engine::command::{friendlist::*, profile_service::*, wtlogin::*};
use crate::client::msg::MsgElem;
use crate::client::structs::{FriendInfo, GroupInfo, GroupMemberInfo};
use crate::jce::{SvcDevLoginInfo, SvcRespRegister};
use crate::{RQError, RQResult};

/// 登录相关
impl super::Client {
    /// 二维码登录 - 获取二维码
    pub async fn fetch_qrcode(&self) -> Result<QRCodeState, RQError> {
        let req = self.engine.read().await.build_qrcode_fetch_request_packet();
        let resp = self.send_and_wait(req).await?;
        let resp = self
            .engine
            .read()
            .await
            .decode_trans_emp_response(resp.body)?;
        self.process_trans_emp_response(resp.clone()).await;
        Ok(resp)
    }

    /// 二维码登录 - 查询二维码状态
    pub async fn query_qrcode_result(&self, sig: &[u8]) -> Result<QRCodeState, RQError> {
        let req = self
            .engine
            .read()
            .await
            .build_qrcode_result_query_request_packet(sig);
        let resp = self.send_and_wait(req).await?;
        let resp = self
            .engine
            .read()
            .await
            .decode_trans_emp_response(resp.body)?;
        self.process_trans_emp_response(resp.clone()).await;
        Ok(resp)
    }

    /// 二维码登录 - 登录 ( 可能还需要 device_lock_login )
    pub async fn qrcode_login(
        &self,
        tmp_pwd: &[u8],
        tmp_no_pic_sig: &[u8],
        tgt_qr: &[u8],
    ) -> Result<LoginResponse, RQError> {
        let req =
            self.engine
                .read()
                .await
                .build_qrcode_login_packet(tmp_pwd, tmp_no_pic_sig, tgt_qr);
        let resp = self.send_and_wait(req).await?;
        let resp = self.engine.read().await.decode_login_response(resp.body)?;
        self.process_login_response(resp.clone()).await;
        Ok(resp)
    }

    /// 密码登录 - 提交密码md5
    pub async fn password_md5_login(
        &self,
        uin: i64,
        password_md5: &[u8],
    ) -> Result<LoginResponse, RQError> {
        self.engine.read().await.uin.store(uin, Ordering::Relaxed);
        let req = self
            .engine
            .read()
            .await
            .build_login_packet(password_md5, true);
        let resp = self.send_and_wait(req).await?;
        let resp = self.engine.read().await.decode_login_response(resp.body)?;
        self.process_login_response(resp.clone()).await;
        Ok(resp)
    }

    pub async fn password_login(&self, uin: i64, password: &str) -> Result<LoginResponse, RQError> {
        self.password_md5_login(uin, &md5::compute(password).to_vec())
            .await
    }

    /// 密码登录 - 请求短信验证码
    pub async fn request_sms(&self) -> Result<LoginResponse, RQError> {
        let req = self.engine.read().await.build_sms_request_packet();
        let resp = self.send_and_wait(req).await?;
        let resp = self.engine.read().await.decode_login_response(resp.body)?;
        self.process_login_response(resp.clone()).await;
        Ok(resp)
    }

    /// 密码登录 - 提交短信验证码
    pub async fn submit_sms_code(&self, code: &str) -> Result<LoginResponse, RQError> {
        let req = self
            .engine
            .read()
            .await
            .build_sms_code_submit_packet(code.trim());
        let resp = self.send_and_wait(req).await?;
        let resp = self.engine.read().await.decode_login_response(resp.body)?;
        self.process_login_response(resp.clone()).await;
        Ok(resp)
    }

    /// 密码登录 - 提交滑块ticket
    pub async fn submit_ticket(&self, ticket: &str) -> Result<LoginResponse, RQError> {
        let req = self.engine.read().await.build_ticket_submit_packet(ticket);
        let resp = self.send_and_wait(req).await?;
        let resp = self.engine.read().await.decode_login_response(resp.body)?;
        self.process_login_response(resp.clone()).await;
        Ok(resp)
    }

    /// 设备锁登录 - 二维码、密码登录都需要
    pub async fn device_lock_login(&self) -> Result<LoginResponse, RQError> {
        let req = self.engine.read().await.build_device_lock_login_packet();
        let resp = self.send_and_wait(req).await?;
        let resp = self.engine.read().await.decode_login_response(resp.body)?;
        self.process_login_response(resp.clone()).await;
        Ok(resp)
    }

    /// token 登录
    pub async fn token_login(&self, mut token: impl Buf) -> RQResult<()> {
        self.load_token(&mut token).await;
        let req = self.engine.read().await.build_request_change_sig_packet();
        self.send_and_wait(req).await?;
        let r = tokio::join! {
            self.wait_packet("StatSvc.ReqMSFOffline", 1),
            self.wait_packet("MessageSvc.PushForceOffline", 1)
        };
        if let (Err(RQError::Timeout), Err(RQError::Timeout)) = r {
            Ok(())
        } else {
            Err(RQError::TokenLoginFailed)
        }
    }

    /// 注册客户端，登录后必须注册
    pub async fn register_client(&self) -> Result<SvcRespRegister, RQError> {
        let req = self.engine.read().await.build_client_register_packet();
        let resp = self.send_and_wait(req).await?;
        let resp = self
            .engine
            .read()
            .await
            .decode_client_register_response(resp.body)?;
        if resp.result != "" || resp.reply_code != 0 {
            return Err(RQError::Other(resp.result + &resp.reply_code.to_string()));
        }
        self.online.store(true, Ordering::SeqCst);
        Ok(resp)
    }

    pub async fn heartbeat(&self) -> RQResult<()> {
        let req = self.engine.read().await.build_heartbeat_packet();
        let _ = self.send_and_wait(req).await?;
        Ok(())
    }
}

/// API
impl super::Client {
    /// 获取进群申请信息
    pub async fn get_group_system_messages(
        &self,
        suspicious: bool,
    ) -> Result<GroupSystemMessages, RQError> {
        let req = self
            .engine
            .read()
            .await
            .build_system_msg_new_group_packet(suspicious);
        let resp = self.send_and_wait(req).await?;
        self.engine
            .read()
            .await
            .decode_system_msg_group_packet(resp.body)
    }

    /// 获取好友列表
    /// 第一个参数offset，从0开始；第二个参数count，150，另外两个都是0
    pub async fn get_friend_list(
        &self,
        friend_start_index: i16,
        friend_list_count: i16,
        group_start_index: i16,
        group_list_count: i16,
    ) -> Result<FriendListResponse, RQError> {
        let req = self
            .engine
            .read()
            .await
            .build_friend_group_list_request_packet(
                friend_start_index,
                friend_list_count,
                group_start_index,
                group_list_count,
            );
        let resp = self.send_and_wait(req).await?;
        self.engine
            .read()
            .await
            .decode_friend_group_list_response(resp.body)
    }

    /// 获取群列表
    /// 第一个参数offset，从0开始；第二个参数count，150，另外两个都是0
    pub async fn get_group_list(&self, vec_cookie: &[u8]) -> Result<GroupListResponse, RQError> {
        let req = self
            .engine
            .read()
            .await
            .build_group_list_request_packet(vec_cookie);
        let resp = self.send_and_wait(req).await?;
        self.engine
            .read()
            .await
            .decode_group_list_response(resp.body)
    }

    /// 发送群消息 TODO 切片, At预处理Display
    pub async fn send_group_message(
        &self,
        group_code: i64,
        message_chain: Vec<MsgElem>,
    ) -> Result<(), RQError> {
        let elems = crate::client::msg::into_elems(message_chain);
        let req = self
            .engine
            .read()
            .await
            .build_group_sending_packet(group_code, 383, 1, 0, 0, false, elems);
        self.send(req).await?;
        Ok(())
    }

    /// 获取群成员信息
    pub async fn get_group_member_info(
        &self,
        group_code: i64,
        uin: i64,
    ) -> Result<GroupMemberInfo, RQError> {
        let req = self
            .engine
            .read()
            .await
            .build_group_member_info_request_packet(group_code, uin);
        let resp = self.send_and_wait(req).await?;
        self.engine
            .read()
            .await
            .decode_group_member_info_response(resp.body)
    }

    /// 通过群号获取群
    pub async fn find_group(&self, code: i64) -> Option<Arc<GroupInfo>> {
        for g in self.group_list.read().await.iter() {
            if g.code == code {
                return Some(g.clone());
            }
        }
        None
    }

    /// 通过群号从服务器获取群，请先尝试 find_group
    pub async fn get_group(&self, code: i64) -> Option<Arc<GroupInfo>> {
        let req = self
            .engine
            .read()
            .await
            .build_group_info_request_packet(code);
        if let Ok(_resp) = self.send_and_wait(req).await {
            // decode_group_info_response(&resp.body)
            todo!()
        } else {
            None
        }
    }

    /// 通过uin获取群
    pub async fn find_group_by_uin(&self, uin: i64) -> Option<Arc<GroupInfo>> {
        for g in self.group_list.read().await.iter() {
            if g.uin == uin {
                return Some(g.clone());
            }
        }
        None
    }

    /// 刷新群列表 TODO 获取群成员列表
    pub async fn reload_group_list(self: &Arc<Self>) -> Result<(), RQError> {
        // 获取群列表
        let mut vec_cookie = Bytes::new();
        let mut groups = Vec::new();
        loop {
            let resp = self.get_group_list(&vec_cookie).await?;
            vec_cookie = resp.vec_cookie;
            for g in resp.groups {
                groups.push(Arc::new(g));
            }
            if vec_cookie.is_empty() {
                break;
            }
        }

        // 对于每个群，获取群成员列表（最多10个群并发执行）
        let semaphore = Arc::new(tokio::sync::Semaphore::new(50));
        let mut handles = Vec::new();

        for g in groups.iter_mut() {
            let cli = self.clone();
            let group = g.clone();
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| RQError::Other("semaphore acquire_owned err".into()))?;
            handles.push(tokio::spawn(async move {
                let mut mem_list = cli
                    .get_group_member_list(group.code, group.uin)
                    .await
                    .ok()?;
                let mut members = group.members.write().await;
                members.append(&mut mem_list);
                drop(permit);
                Some(())
            }));
        }
        for h in handles {
            h.await
                .map_err(|_| RQError::Other("joinhandle err".into()))?; //todo
        }

        let mut group_list = self.group_list.write().await;
        group_list.clear();
        group_list.append(&mut groups); // TODO 不知道会不会复制大量内存
        Ok(())
    }

    /// 刷新好友列表
    pub async fn reload_friend_list(&self) -> Result<(), RQError> {
        let mut cur_friend_count = 0;
        let mut friends = Vec::new();
        loop {
            let resp = self.get_friend_list(cur_friend_count, 150, 0, 0).await?;
            cur_friend_count += resp.list.len() as i16;
            for f in resp.list {
                friends.push(Arc::new(f));
            }
            if cur_friend_count >= resp.total_count {
                break;
            }
        }
        let mut friend_list = self.friend_list.write().await;
        friend_list.clear();
        friend_list.append(&mut friends);
        Ok(())
    }

    /// 根据 uin 获取好友
    pub async fn find_friend(&self, uin: i64) -> Option<Arc<FriendInfo>> {
        self.friend_list
            .read()
            .await
            .iter()
            .find(|f| f.uin == uin)
            .cloned()
    }

    /// 获取群成员列表 (low level api)
    async fn _get_group_member_list(
        &self,
        group_uin: i64,
        group_code: i64,
        next_uin: i64,
    ) -> Result<GroupMemberListResponse, RQError> {
        let req = self
            .engine
            .read()
            .await
            .build_group_member_list_request_packet(group_uin, group_code, next_uin);
        let resp = self.send_and_wait(req).await?;
        self.engine
            .read()
            .await
            .decode_group_member_list_response(resp.body)
    }

    /// 获取群成员列表
    pub async fn get_group_member_list(
        &self,
        group_code: i64,
        group_uin: i64,
    ) -> Result<Vec<GroupMemberInfo>, RQError> {
        let mut next_uin = 0;
        let mut list = Vec::new();
        loop {
            let mut resp = self
                ._get_group_member_list(group_uin, group_code, next_uin)
                .await?;
            if resp.list.is_empty() {
                return Err(RQError::Other("member list is empty".to_string()));
            }
            for m in resp.list.iter_mut() {
                m.group_code = group_code;
            }
            list.append(&mut resp.list);
            next_uin = resp.next_uin;
            if next_uin == 0 {
                break;
            }
        }
        Ok(list)
    }

    /// 刷新客户端状态
    pub async fn refresh_status(&self) -> Result<(), RQError> {
        let req = self
            .engine
            .read()
            .await
            .build_get_offline_msg_request_packet(self.last_message_time.load(Ordering::SeqCst));
        let _resp = self.send_and_wait(req).await?;
        Ok(())
    }

    /// 标记群消息已读
    pub async fn mark_group_message_readed(
        &self,
        group_code: i64,
        seq: i32,
    ) -> Result<(), RQError> {
        let req = self
            .engine
            .read()
            .await
            .build_group_msg_readed_packet(group_code, seq);
        let _resp = self.send_and_wait(req).await?;
        Ok(())
    }

    /// 标记私聊消息已读 TODO 待测试
    pub async fn mark_private_message_readed(&self, _uin: i64, _time: i64) -> Result<(), RQError> {
        // let resp = self
        //     .send_and_wait(self.build_private_msg_read_packet(uin, time).await)
        //     .await?;
        // println!("{}", resp.command_name); // todo
        Ok(())
    }

    /// 获取通过安全验证的设备
    pub async fn get_allowed_clients(&self) -> Result<Vec<SvcDevLoginInfo>, RQError> {
        let req = self.engine.read().await.build_device_list_request_packet();
        let resp = self.send_and_wait(req).await?;
        self.engine.read().await.decode_dev_list_response(resp.body)
    }
}
